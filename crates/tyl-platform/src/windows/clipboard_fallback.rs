//! Clipboard fallback channel: simulated Ctrl+C with full snapshot/restore.
//!
//! Windows 剪贴板兼容取词流程：
//!
//! 1. 还欠账 —— 上次还原失败时剪贴板残留的是我们的污染物，必须先
//!    补还原，否则会把污染物当用户内容快照（滚雪球 bug）。
//! 2. 等输入静默 —— 鼠标键释放后选区才确定；热键紧跟松手时注入的
//!    Ctrl+C 会与选择余波交错（WPS 两轮写剪贴板）。
//! 3. 等冲突修饰键释放 —— 不在 Alt/Shift/Win 仍被按住时注入任何按键，
//!    避免 Edge 等应用切到菜单或清掉原选区；Ctrl 可直接复用。
//! 4. 快照 —— 在等待完成后立即用 OleGetClipboard 备份可还原的格式，避免
//!    用等待前的旧快照覆盖期间发生的合法剪贴板写入。
//! 5. 模拟 Ctrl+C —— 冲突修饰键已经释放，发送唯一一次干净的 Ctrl+C。
//! 6. 读文本时追加 Ditto 和系统历史排除标记。源应用首次发布的复制事件先于标记。
//! 7. 持有剪贴板锁时核对序列号，再恢复快照。仅对同一次写入重试。

use std::time::{Duration, Instant};

use tyl_core::{CaptureAnchor, CaptureError, CaptureSource, CapturedText, TextCapture};

use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, CLIPBRD_E_CANT_OPEN, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::Com::{
    CoInitializeEx, IDataObject, COINIT_APARTMENTTHREADED, FORMATETC, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::System::Ole::{OleGetClipboard, ReleaseStgMedium, CF_LOCALE, CF_UNICODETEXT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    VK_CONTROL, VK_INSERT, VK_LCONTROL, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RWIN, VK_SHIFT,
};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MODIFIER_RELEASE_TIMEOUT: Duration = Duration::from_secs(3);
const MODIFIER_POLL_INTERVAL: Duration = Duration::from_millis(5);
const CLIPBOARD_FORMAT: u32 = CF_UNICODETEXT.0 as u32;

/// 日志钩子：宿主（tyl-app）注入写文件实现；默认 stderr。
static LOG_HOOK: std::sync::Mutex<Option<fn(&str)>> = std::sync::Mutex::new(None);

/// 设置日志钩子（幂等）。在应用启动早期调用。
pub fn set_log_hook(hook: fn(&str)) {
    *LOG_HOOK.lock().unwrap() = Some(hook);
}

fn log_str(msg: &str) {
    match *LOG_HOOK.lock().unwrap() {
        Some(h) => h(msg),
        None => eprintln!("[tyl] {msg}"),
    }
}

pub(crate) fn trace_log(msg: &str) {
    log_str(msg);
}

struct SnapshotEntry {
    format: u32,
    bytes: Vec<u8>,
}

/// Snapshot of every HGLOBAL clipboard format at a moment in time.
struct ClipboardSnapshot {
    entries: Vec<SnapshotEntry>,
    complete: bool,
}

pub struct ClipboardFallback {
    /// When false (CLI `--no-restore` / user setting), skip the restore step.
    pub restore: bool,
    /// 还原失败的"欠账"：上一次没还回去的用户快照。存在期间剪贴板
    /// 残留的是我们的污染物，下次取词必须先还账（见模块注释步骤 1）。
    debt: std::sync::Mutex<Option<(ClipboardSnapshot, u32)>>,
    /// 剪贴板取词是一个不可重入事务；并发快照/还原会互相覆盖。
    operation: std::sync::Mutex<()>,
}

impl ClipboardFallback {
    pub fn new(restore: bool) -> Self {
        Self {
            restore,
            debt: std::sync::Mutex::new(None),
            operation: std::sync::Mutex::new(()),
        }
    }

    /// 启动预热只触发 COM/OLE 和剪贴板格式枚举的惰性初始化。
    ///
    /// 这里绝不能复用 `capture()`：完整取词会模拟 Ctrl+C。若应用正从
    /// PowerShell/cmd 启动，这个按键会发给前台终端并中断正在运行的
    /// tyl-app，表现为启动窗口一闪而退；取到选区时还会污染剪贴板。
    pub fn warmup() {
        ensure_com_on_this_thread();
        match snapshot_via_ole() {
            Ok(snapshot) => log_str(&format!(
                "[clip] 只读预热完成: {} 个格式",
                snapshot.entries.len()
            )),
            Err(e) => log_str(&format!("[clip] 只读预热失败（不影响后续取词）: {e}")),
        }
    }
}

impl Default for ClipboardFallback {
    fn default() -> Self {
        Self::new(true)
    }
}

impl TextCapture for ClipboardFallback {
    fn capture(
        &self,
        _anchor: &CaptureAnchor,
        deadline: Duration,
    ) -> Result<CapturedText, CaptureError> {
        self.capture_guarded(deadline, None)
    }
}

impl ClipboardFallback {
    pub(super) fn capture_guarded(
        &self,
        deadline: Duration,
        current: Option<&dyn Fn() -> bool>,
    ) -> Result<CapturedText, CaptureError> {
        if current.is_some_and(|valid| !valid()) {
            return Err(CaptureError::NoText("selection is no longer active".into()));
        }
        let _operation = self.operation.try_lock().map_err(|_| {
            CaptureError::Channel(
                "another clipboard capture is already in progress; overlapping request skipped"
                    .into(),
            )
        })?;
        let start = Instant::now();
        let capture_budget = deadline;
        let input_quiet_deadline = start + capture_budget;

        // COM 必须就绪：OleGetClipboard/EnumFormatEtc 要求线程已初始化，
        // 未初始化时格式枚举不完整 → 还原丢格式（真机首发 case）。
        ensure_com_on_this_thread();

        let window = ClipboardWindow::new()?;
        let source = CopyTarget::foreground()?;

        // ── 1. 还欠账（上次还原失败？）─────────────────────────────
        {
            let mut debt = self.debt.lock().unwrap();
            if let Some((owed, sequence)) = debt.take() {
                if let Err(retry_sequence) = restore_while(window.0, &owed, sequence, None) {
                    *debt = Some((owed, retry_sequence));
                    return Err(CaptureError::Channel(
                        "上一次取词的剪贴板还原仍未完成".into(),
                    ));
                }
            }
        }

        // ── 2. 输入静默门槛 ────────────────────────────────────────
        wait_mouse_buttons_released(input_quiet_deadline);

        // ── 3. 等待冲突修饰键释放 ──────────────────────────────────
        // UIA 已经在 Pressed 阶段立即尝试过。剪贴板通道只等待会改变
        // Ctrl+C 语义的 Alt/Shift/Win；Ctrl 本身可直接复用，因此 Ctrl-only
        // 快捷键仍可在 Pressed 阶段完成取词。直接读取物理键状态，避免再
        // 等 global-shortcut 的 Released 通知带来的调度延迟。
        if conflicting_modifier_down() {
            log_str("[clip] 等待冲突修饰键真实释放后再复制");
            let wait_started = Instant::now();
            let release_deadline = wait_started + MODIFIER_RELEASE_TIMEOUT;
            while conflicting_modifier_down() && Instant::now() < release_deadline {
                std::thread::sleep(MODIFIER_POLL_INTERVAL);
            }
            if conflicting_modifier_down() {
                log_str("[clip] 等待冲突修饰键释放超时，未向目标应用注入按键");
                return Err(CaptureError::NoText(
                    "a conflicting shortcut modifier remained pressed; clipboard copy was skipped"
                        .into(),
                ));
            }
            log_str(&format!(
                "[clip] 冲突修饰键已释放，已等待 {}ms，开始复制",
                wait_started.elapsed().as_millis()
            ));
        }

        // ── 4. 快照当前内容（紧邻模拟复制）─────────────────────────
        // 必须在等待松键之后快照；否则等待期间的合法剪贴板写入会被旧
        // 快照覆盖。
        let (snapshot, seq0) = snapshot_before_copy(self.restore).map_err(CaptureError::Channel)?;
        if !source.still_focused()
            || current.is_some_and(|valid| !valid() || key_down(VK_CONTROL))
            || unsafe { GetClipboardSequenceNumber() } != seq0
        {
            return Err(CaptureError::NoText(
                "selection or clipboard changed before copy".into(),
            ));
        }
        if !send_ctrl_c() {
            return Err(CaptureError::Channel(
                "failed to inject the copy shortcut".into(),
            ));
        }

        // ── 6. 读文本 + 隐身标记 ───────────────────────────────────
        let response_deadline = Instant::now() + capture_budget.max(Duration::from_millis(500));
        let (captured, sequence) = loop {
            if unsafe { GetClipboardSequenceNumber() } != seq0 {
                if let Some(sample) = read_copy(window.0, &source, seq0, None)? {
                    break sample;
                }
            }
            if Instant::now() >= response_deadline {
                let sequence = unsafe { GetClipboardSequenceNumber() };
                if self.restore && sequence != seq0 && source.owns_clipboard() {
                    self.restore_copy(window.0, snapshot, sequence);
                }
                return Err(CaptureError::NoText(
                    "clipboard channel produced no text".into(),
                ));
            }
            std::thread::sleep(POLL_INTERVAL);
        };

        let restored = self.restore && self.restore_copy(window.0, snapshot, sequence);
        Ok(CapturedText {
            text: captured,
            editable: None,
            selection_id: None,
            source: CaptureSource::Clipboard { restored },
            selection_rect: None, // no geometry from this channel
            elapsed: start.elapsed(),
        })
    }
    fn restore_copy(&self, owner: HWND, snapshot: ClipboardSnapshot, sequence: u32) -> bool {
        match restore_while(owner, &snapshot, sequence, None) {
            Ok(restored) => restored,
            Err(retry_sequence) => {
                *self.debt.lock().unwrap() = Some((snapshot, retry_sequence));
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// COM / 输入
// ---------------------------------------------------------------------------

/// 线程级一次性 COM 初始化（幂等：S_OK 首次 / S_FALSE 已初始化均算成功）。
/// ClipboardFallback 的调用者（pipeline 工作线程）不保证初始化过 COM。
fn ensure_com_on_this_thread() {
    use std::cell::Cell;
    thread_local! {
        static COM_READY: Cell<bool> = const { Cell::new(false) };
    }
    COM_READY.with(|ready| {
        if ready.get() {
            return;
        }
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // RPC_E_CHANGED_MODE = 已按别的模式初始化，OLE 调用仍可用。
        ready.set(hr.is_ok() || hr.0 == -2147417850i32);
    });
}

/// 会改变 Ctrl+C/Ctrl+Insert 含义的物理修饰键。
///
/// Ctrl 单独处理：用户已经按住 Ctrl 时直接复用，不先释放再重新注入。
const CONFLICTING_MODIFIER_VKS: [VIRTUAL_KEY; 4] = [VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];

fn key_down(vk: VIRTUAL_KEY) -> bool {
    (unsafe { GetAsyncKeyState(vk.0 as i32) } as u16) & 0x8000 != 0
}

fn conflicting_modifier_down() -> bool {
    CONFLICTING_MODIFIER_VKS.iter().copied().any(key_down)
}

/// 等待物理鼠标按键全部释放（选区确定），上限 150ms。
/// 热键紧跟鼠标松开时，注入的 Ctrl+C 会与选择余波交错——目标应用在
/// 选区未定时复制会分多轮写剪贴板，破坏还原（真机高频 case）。
fn wait_mouse_buttons_released(deadline: Instant) {
    const VK_LBUTTON: i32 = 0x01;
    const VK_RBUTTON: i32 = 0x02;
    let limit = deadline.min(Instant::now() + Duration::from_millis(150));
    while Instant::now() < limit {
        let held = (unsafe { GetAsyncKeyState(VK_LBUTTON) } as u16) & 0x8000 != 0
            || (unsafe { GetAsyncKeyState(VK_RBUTTON) } as u16) & 0x8000 != 0;
        if !held {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // 超时（用户长按拖选？）也继续——取词比等待更重要，还原尽力而为。
}

fn send_ctrl_c() -> bool {
    send_copy_shortcut(VIRTUAL_KEY(0x43))
}

#[derive(Clone, Copy)]
struct KeyStroke {
    key: VIRTUAL_KEY,
    up: bool,
}

fn copy_key_strokes(
    copy_key: VIRTUAL_KEY,
    physical_ctrl: bool,
    physical_copy_key: bool,
) -> Vec<KeyStroke> {
    let mut strokes = Vec::with_capacity(5);
    if physical_copy_key {
        strokes.push(KeyStroke {
            key: copy_key,
            up: true,
        });
    }
    if !physical_ctrl {
        strokes.push(KeyStroke {
            key: VK_CONTROL,
            up: false,
        });
    }
    strokes.push(KeyStroke {
        key: copy_key,
        up: false,
    });
    strokes.push(KeyStroke {
        key: copy_key,
        up: true,
    });
    if !physical_ctrl {
        strokes.push(KeyStroke {
            key: VK_CONTROL,
            up: true,
        });
    }
    strokes
}

fn send_copy_shortcut(copy_key: VIRTUAL_KEY) -> bool {
    let mk = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: if vk.0 == 0x43 { 0x2E } else { 0 },
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    // 永不合成 Alt/Shift/Win KeyUp：Edge 等应用可能因此激活菜单、移动
    // 焦点或清空原选区。取词入口已经等待这些按键真实释放；这里再次
    // 检查是为了封住检测与 SendInput 之间极短的竞态窗口。
    if conflicting_modifier_down() {
        log_str("[clip] 注入前检测到冲突修饰键，已安全取消复制");
        return false;
    }
    let physical_ctrl = key_down(VK_CONTROL) || key_down(VK_LCONTROL) || key_down(VK_RCONTROL);
    let physical_copy_key = key_down(copy_key);

    // A user-configured hotkey may itself contain C/Insert. Normalize that key
    // before emitting a fresh copy chord, again without synthesizing it back.
    let strokes = copy_key_strokes(copy_key, physical_ctrl, physical_copy_key);
    let seq: Vec<INPUT> = strokes
        .iter()
        .map(|stroke| mk(stroke.key, stroke.up))
        .collect();
    log_str(&format!(
        "[clip] 注入复制: key={} Ctrl={}",
        if copy_key == VK_INSERT { "Insert" } else { "C" },
        if physical_ctrl {
            "复用物理按键"
        } else {
            "合成"
        }
    ));
    unsafe {
        let sent = SendInput(&seq, std::mem::size_of::<INPUT>() as i32);
        if sent != seq.len() as u32 {
            let mut releases = vec![mk(copy_key, true)];
            if !physical_ctrl {
                releases.push(mk(VK_CONTROL, true));
            }
            SendInput(&releases, std::mem::size_of::<INPUT>() as i32);
            return false;
        }
        true
    }
}

pub struct SelectionVerification {
    snapshot: Option<ClipboardSnapshot>,
    expected: String,
    sequence: u32,
    owner: usize,
}

impl Drop for SelectionVerification {
    fn drop(&mut self) {
        if let Some(snapshot) = self.snapshot.take() {
            let _ = restore_while(
                HWND(self.owner as *mut _),
                &snapshot,
                self.sequence,
                Some(&self.expected),
            );
        }
    }
}

/// A replacement payload that is invisible to cooperating clipboard history
/// managers. Dropping the guard restores the user's previous clipboard only
/// while both the clipboard sequence and text still identify our write.
pub struct HiddenClipboardReplacement {
    snapshot: Option<ClipboardSnapshot>,
    expected: String,
    sequence: u32,
    owner: usize,
}

impl HiddenClipboardReplacement {
    pub fn restore(mut self) -> bool {
        self.restore_if_unchanged()
    }

    fn restore_if_unchanged(&mut self) -> bool {
        let Some(snapshot) = self.snapshot.take() else {
            return true;
        };
        let restored = restore_while(
            HWND(self.owner as *mut _),
            &snapshot,
            self.sequence,
            Some(&self.expected),
        )
        .unwrap_or(false);
        log_str(if restored {
            "[replace] 已恢复替换前的剪贴板"
        } else {
            "[replace] 未恢复剪贴板（已被更新或暂被占用）"
        });
        restored
    }
}

impl Drop for HiddenClipboardReplacement {
    fn drop(&mut self) {
        let _ = self.restore_if_unchanged();
    }
}

fn clipboard_text_matches(expected: &str, actual: &str) -> bool {
    expected == actual
        || expected.replace("\r\n", "\n").replace('\r', "\n")
            == actual.replace("\r\n", "\n").replace('\r', "\n")
}

/// Verify a still-selected range in editors whose UIA provider becomes
/// unavailable after focus leaves and returns. The previous clipboard is
/// restored on failure or if the returned guard is dropped before it is
/// transferred into a hidden replacement transaction.
pub fn verify_selected_text(
    owner: usize,
    expected: &str,
    timeout: Duration,
    still_current: impl Fn() -> bool,
) -> Result<SelectionVerification, String> {
    if expected.trim().is_empty() {
        return Err("原选区为空，请重新划词".into());
    }
    ensure_com_on_this_thread();
    let source = CopyTarget::foreground().map_err(|e| e.to_string())?;
    let (snapshot, sequence) = snapshot_before_copy(true)?;
    // Some editors do not republish a selection if it matches the clipboard.
    // A unique hidden probe makes a fresh copy observable without trusting stale
    // contents. The original clipboard belongs to the guard for every exit path.
    static PROBE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let probe = format!(
        "TYL-copy-probe-{}-{}",
        std::process::id(),
        PROBE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    if !still_current() {
        return Err("焦点已离开原输入控件，请重新划词".into());
    }
    let initial_sequence = write_hidden_text(owner, &probe, sequence, &snapshot)?;
    let mut guard = SelectionVerification {
        snapshot: Some(snapshot),
        expected: probe,
        sequence: initial_sequence,
        owner,
    };
    if !still_current() {
        return Err("焦点已离开原输入控件，请重新划词".into());
    }
    if !send_copy_shortcut(VIRTUAL_KEY(0x43)) {
        return Err("系统未能发送复制按键，请检查两应用是否以相同权限运行".into());
    }

    let deadline = Instant::now() + timeout;
    let mut last_sequence = initial_sequence;
    let mut last_change = Instant::now();
    let retry_at = Instant::now() + timeout / 2;
    let mut retried = false;
    let mut matched = false;
    while Instant::now() < deadline {
        std::thread::sleep(POLL_INTERVAL);
        if !still_current() {
            return Err("焦点已离开原输入控件，请重新划词".into());
        }
        let sequence = unsafe { GetClipboardSequenceNumber() };
        if sequence == initial_sequence {
            if !retried && Instant::now() >= retry_at {
                retried = true;
                log_str("[replace] Ctrl+C 未更新剪贴板，尝试 Ctrl+Insert");
                if !send_copy_shortcut(VK_INSERT) {
                    return Err("系统未能发送复制按键，请检查两应用是否以相同权限运行".into());
                }
            }
            continue;
        }
        if sequence != last_sequence {
            last_sequence = sequence;
            last_change = Instant::now();
        }
        if sequence != guard.sequence {
            let Some((text, marked_sequence)) = read_copy(
                HWND(owner as *mut _),
                &source,
                guard.sequence,
                Some(expected),
            )
            .map_err(|e| e.to_string())?
            else {
                continue;
            };
            guard.expected = text;
            guard.sequence = marked_sequence;
            last_sequence = marked_sequence;
            matched = true;
        }
        // WPS may publish clipboard formats in more than one pass. Wait for a
        // short quiet period so its later write cannot overwrite the translation.
        if matched && last_change.elapsed() >= Duration::from_millis(90) {
            break;
        }
    }

    if matched {
        Ok(guard)
    } else {
        log_str("[replace] 两种复制快捷键均未返回原选区");
        Err("原应用未返回选中文本，选区可能已在失焦后消失；请重新选中原文后再试".into())
    }
}

/// Back up the current clipboard (or reuse the snapshot taken while verifying
/// the selection), then publish the replacement text and history-ignore marker
/// in one OpenClipboard session. Clipboard listeners only observe the complete
/// payload after CloseClipboard, so Ditto and Win+V can ignore it reliably.
pub fn prepare_hidden_replacement(
    owner: usize,
    text: &str,
    mut verification: Option<SelectionVerification>,
) -> Result<HiddenClipboardReplacement, String> {
    if text.trim().is_empty() {
        return Err("还没有可替换的译文".into());
    }
    ensure_com_on_this_thread();
    let (snapshot, sequence) = if let Some(guard) = verification.as_mut() {
        let snapshot = guard.snapshot.as_ref().ok_or("原剪贴板快照已失效")?;
        let sequence = write_hidden_text(owner, text, guard.sequence, snapshot)?;
        (guard.snapshot.take().unwrap(), sequence)
    } else {
        let (snapshot, previous) = snapshot_before_copy(true)?;
        let sequence = write_hidden_text(owner, text, previous, &snapshot)?;
        (snapshot, sequence)
    };
    Ok(HiddenClipboardReplacement {
        snapshot: Some(snapshot),
        expected: text.into(),
        sequence,
        owner,
    })
}

// ---------------------------------------------------------------------------
// 隐身标记（剪贴板管理器协作）
// ---------------------------------------------------------------------------

/// Called with the clipboard open. Set both markers before publishing any TYL-owned text.
fn exclude_from_history_locked() -> bool {
    let mut marked = true;
    for name in [
        w!("Clipboard Viewer Ignore"),
        w!("ExcludeClipboardContentFromMonitorProcessing"),
    ] {
        unsafe {
            let format = RegisterClipboardFormatW(name);
            marked &= format != 0
                && (IsClipboardFormatAvailable(format).is_ok()
                    || set_bytes_locked(format, &0u32.to_le_bytes()));
        }
    }
    marked
}

// The clipboard takes ownership of the handle only when SetClipboardData succeeds.
fn set_bytes_locked(format: u32, bytes: &[u8]) -> bool {
    let Some(handle) = bytes_to_hglobal(bytes) else {
        return false;
    };
    unsafe {
        if SetClipboardData(format, Some(HANDLE(handle.0))).is_ok() {
            return true;
        }
        let _ = GlobalFree(Some(handle));
    }
    false
}

fn write_hidden_text(
    owner: usize,
    text: &str,
    expected: u32,
    snapshot: &ClipboardSnapshot,
) -> Result<u32, String> {
    let window = HWND(owner as *mut _);
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    for _ in 0..15 {
        unsafe {
            if OpenClipboard(Some(window)).is_ok() {
                let result = if GetClipboardSequenceNumber() != expected {
                    Err("剪贴板已更新，请重试")
                } else if EmptyClipboard().is_err() {
                    Err("无法清空剪贴板，请重试")
                } else if exclude_from_history_locked()
                    && set_bytes_locked(CLIPBOARD_FORMAT, &bytes)
                {
                    Ok(())
                } else {
                    let restored = restore_locked(snapshot);
                    Err(if restored {
                        "写入临时文本失败，已恢复剪贴板"
                    } else {
                        "写入临时文本失败，剪贴板未能完整恢复"
                    })
                };
                let _ = CloseClipboard();
                result?;
                // Closing can add synthesized text formats and advance the sequence.
                let published = GetClipboardSequenceNumber();
                if GetClipboardOwner().ok() != Some(window) {
                    return Err("剪贴板已被其他应用更新".into());
                }
                return Ok(published);
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    Err("剪贴板正被占用，请重试".into())
}

// ---------------------------------------------------------------------------
// 读 / 快照 / 还原
// ---------------------------------------------------------------------------

// EmptyClipboard needs a real window to take ownership before SetClipboardData.
struct ClipboardWindow(HWND);

impl ClipboardWindow {
    fn new() -> Result<Self, CaptureError> {
        use windows::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            CreateWindowExW(
                Default::default(),
                w!("STATIC"),
                None,
                Default::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .map(Self)
            .map_err(|e| CaptureError::Channel(format!("clipboard owner window: {e}")))
        }
    }
}

impl Drop for ClipboardWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

// Hosted controls may belong to a different process from the foreground frame.
struct CopyTarget {
    window: HWND,
    frame_pid: u32,
    focus_pid: u32,
}

impl CopyTarget {
    fn foreground() -> Result<Self, CaptureError> {
        use windows::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            let window = GetForegroundWindow();
            let mut frame_pid = 0;
            let thread = GetWindowThreadProcessId(window, Some(&mut frame_pid));
            if thread == 0 {
                return Err(CaptureError::NoText("no foreground copy target".into()));
            }
            let mut info = GUITHREADINFO {
                cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            let mut focus_pid = 0;
            if GetGUIThreadInfo(thread, &mut info).is_ok() {
                GetWindowThreadProcessId(info.hwndFocus, Some(&mut focus_pid));
            }
            Ok(Self {
                window,
                frame_pid,
                focus_pid,
            })
        }
    }
    fn still_focused(&self) -> bool {
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == self.window }
    }
    fn owns_clipboard(&self) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
        unsafe {
            let Ok(owner) = GetClipboardOwner() else {
                return false;
            };
            let mut pid = 0;
            GetWindowThreadProcessId(owner, Some(&mut pid));
            pid != 0 && (pid == self.frame_pid || pid == self.focus_pid)
        }
    }
}

// Observe the owner, text and post-marker sequence under one lock.
fn read_copy(
    owner: HWND,
    source: &CopyTarget,
    previous: u32,
    expected: Option<&str>,
) -> Result<Option<(String, u32)>, CaptureError> {
    unsafe {
        if OpenClipboard(Some(owner)).is_err() {
            return Ok(None);
        }
        let result = if GetClipboardSequenceNumber() == previous {
            Ok(None)
        } else if !source.owns_clipboard() {
            Err(CaptureError::NoText(
                "clipboard write could not be attributed to the copy target".into(),
            ))
        } else if let Some(text) = clipboard_text_locked().filter(|s| !s.trim().is_empty()) {
            if expected.is_some_and(|original| !clipboard_text_matches(original, &text)) {
                Err(CaptureError::NoText("原选区已变化，请重新划词".into()))
            } else {
                if !exclude_from_history_locked() {
                    log_str("[clip] 部分剪贴板历史排除标记写入失败");
                }
                Ok(Some((text, GetClipboardSequenceNumber())))
            }
        } else {
            Ok(None)
        };
        let _ = CloseClipboard();
        result
    }
}

// Caller holds the clipboard lock.
fn clipboard_text_locked() -> Option<String> {
    unsafe {
        let handle = GetClipboardData(CLIPBOARD_FORMAT).ok()?;
        utf16_bytes_to_string(&hglobal_bytes(HGLOBAL(handle.0))?)
    }
}

// Native handles and owner-managed private data cannot be restored from copied bytes.
fn can_snapshot_format(format: u16) -> bool {
    use windows::Win32::System::Ole::*;
    !matches!(
        CLIPBOARD_FORMAT(format),
        CF_BITMAP
            | CF_METAFILEPICT
            | CF_PALETTE
            | CF_ENHMETAFILE
            | CF_OWNERDISPLAY
            | CF_DSPBITMAP
            | CF_DSPMETAFILEPICT
            | CF_DSPENHMETAFILE
    ) && !(CF_PRIVATEFIRST.0..=CF_PRIVATELAST.0).contains(&format)
}

fn snapshot_via_ole() -> windows::core::Result<ClipboardSnapshot> {
    unsafe {
        let data: IDataObject = OleGetClipboard()?;
        let enum_fmt = data.EnumFormatEtc(1 /* DATADIR_GET */)?;

        let mut entries = Vec::new();
        let mut complete = true;
        let mut batch = [FORMATETC::default(); 8];
        loop {
            let mut fetched = 0u32;
            if enum_fmt.Next(&mut batch, Some(&mut fetched)).is_err() {
                complete = false;
                break;
            }
            if fetched == 0 {
                break;
            }
            for fe in &batch[..fetched as usize] {
                if !can_snapshot_format(fe.cfFormat) {
                    complete = false;
                    continue;
                }
                let before = entries.len();
                let mut request = *fe;
                request.tymed = TYMED_HGLOBAL.0 as u32;
                match data.GetData(&request) {
                    Ok(mut medium) => {
                        if medium.tymed == TYMED_HGLOBAL.0 as u32 {
                            if let Some(bytes) = hglobal_bytes(HGLOBAL(medium.u.hGlobal.0)) {
                                entries.push(SnapshotEntry {
                                    format: request.cfFormat as u32,
                                    bytes,
                                });
                            }
                        }
                        ReleaseStgMedium(&mut medium);
                    }
                    Err(e) if e.code() == CLIPBRD_E_CANT_OPEN => return Err(e),
                    Err(_) => {}
                }
                // OLE can advertise CF_LOCALE while rendering it only through the Win32 clipboard.
                let format = u32::from(fe.cfFormat);
                if entries.len() == before
                    && format == u32::from(CF_LOCALE.0)
                    && OpenClipboard(None).is_ok()
                {
                    let locale = GetClipboardData(format)
                        .ok()
                        .and_then(|handle| hglobal_bytes(HGLOBAL(handle.0)));
                    let _ = CloseClipboard();
                    if let Some(bytes) = locale {
                        entries.push(SnapshotEntry { format, bytes });
                    } else if IsClipboardFormatAvailable(format).is_err() {
                        continue;
                    }
                }
                complete &= entries.len() > before;
            }
            if fetched < batch.len() as u32 {
                break;
            }
        }
        Ok(ClipboardSnapshot { entries, complete })
    }
}

fn snapshot_before_copy(restore: bool) -> Result<(ClipboardSnapshot, u32), String> {
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let deadline = Instant::now() + Duration::from_millis(120);
    let snapshot = loop {
        match snapshot_via_ole() {
            Ok(snapshot) => break snapshot,
            Err(e) if e.code() == CLIPBRD_E_CANT_OPEN && Instant::now() < deadline => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(e) => return Err(format!("无法备份剪贴板: {e}")),
        }
    };
    if restore && !snapshot.complete {
        return Err("无法完整备份当前剪贴板，已跳过取词".into());
    }
    if unsafe { GetClipboardSequenceNumber() } != sequence {
        return Err("备份期间剪贴板已更新，请重试".into());
    }
    Ok((snapshot, sequence))
}

// Ok(false) means a newer copy won. Only a failed restore of our own copy is retried.
fn restore_while(
    owner: HWND,
    snapshot: &ClipboardSnapshot,
    sequence: u32,
    expected: Option<&str>,
) -> Result<bool, u32> {
    for _ in 0..12 {
        unsafe {
            if OpenClipboard(Some(owner)).is_ok() {
                if GetClipboardSequenceNumber() != sequence
                    || expected.is_some_and(|text| clipboard_text_locked().as_deref() != Some(text))
                {
                    let _ = CloseClipboard();
                    return Ok(false);
                }
                let verified = restore_locked(snapshot) && verify_restored_locked(snapshot);
                let before_close = GetClipboardSequenceNumber();
                let _ = CloseClipboard();
                if verified {
                    return Ok(true);
                }
                let published = GetClipboardSequenceNumber();
                return if published == before_close || GetClipboardOwner().ok() == Some(owner) {
                    Err(published)
                } else {
                    Ok(false)
                };
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(sequence)
}

/// 打开剪贴板后的写入段。true = 所有格式全部放回成功。
/// SetClipboardData 失败（句柄所有权未转移）必须计入失败——吞掉它
/// 就是谎报"已还原"。
unsafe fn restore_locked(snapshot: &ClipboardSnapshot) -> bool {
    if EmptyClipboard().is_err() {
        return false;
    }
    let mut ok = true;
    for entry in &snapshot.entries {
        ok &= set_bytes_locked(entry.format, &entry.bytes);
    }
    ok
}

/// 回读验证：当前剪贴板文本 == 快照的 CF_UNICODETEXT。
/// 无文本时以逐格式的 SetClipboardData 结果为准。
fn verify_restored_locked(snapshot: &ClipboardSnapshot) -> bool {
    match snapshot
        .entries
        .iter()
        .find(|e| e.format == CLIPBOARD_FORMAT)
        .and_then(|e| utf16_bytes_to_string(&e.bytes))
    {
        Some(expected) => clipboard_text_locked()
            .map(|t| t == expected)
            .unwrap_or(false),
        None => true,
    }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// CF_UNICODETEXT ends at the first NUL, independent of the HGLOBAL allocation size.
fn utf16_bytes_to_string(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let (chunks, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = chunks
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&unit| unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// HGLOBAL 内容深拷贝（GlobalLock/Size 拷出，不夺所有权）。
fn hglobal_bytes(handle: HGLOBAL) -> Option<Vec<u8>> {
    unsafe {
        let ptr = GlobalLock(handle) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, GlobalSize(handle)).to_vec();
        let _ = GlobalUnlock(handle);
        Some(bytes)
    }
}

fn bytes_to_hglobal(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()).ok()?;
        let ptr = GlobalLock(handle) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(handle));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(handle);
        Some(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::{clipboard_text_matches, copy_key_strokes};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_CONTROL};

    #[test]
    fn unicode_text_excludes_allocation_padding() {
        let bytes = [b'A', 0, 0, 0, b'B', 0];
        assert_eq!(super::utf16_bytes_to_string(&bytes).as_deref(), Some("A"));
    }

    #[test]
    fn snapshot_rejects_handle_formats_and_accepts_packed_data() {
        use windows::Win32::System::Ole::*;
        for format in [
            CF_BITMAP,
            CF_METAFILEPICT,
            CF_OWNERDISPLAY,
            CF_PRIVATEFIRST,
            CF_PRIVATELAST,
        ] {
            assert!(!super::can_snapshot_format(format.0));
        }
        for format in [CF_UNICODETEXT, CF_DIB, CF_HDROP] {
            assert!(super::can_snapshot_format(format.0));
        }
        assert!(super::can_snapshot_format(0xc000)); // registered HTML or custom HGLOBAL
    }

    #[test]
    fn replacement_validation_only_normalizes_line_endings() {
        assert!(clipboard_text_matches("a\r\nb", "a\nb"));
        assert!(clipboard_text_matches("a\rb", "a\nb"));
        assert!(!clipboard_text_matches("a b", "a  b"));
        assert!(!clipboard_text_matches("a\n", "a"));
    }

    #[test]
    fn copy_sequence_synthesizes_a_clean_control_chord() {
        let copy_key = VIRTUAL_KEY(0x43);
        let strokes = copy_key_strokes(copy_key, false, false);
        let actual: Vec<(u16, bool)> = strokes.iter().map(|s| (s.key.0, s.up)).collect();

        assert_eq!(
            actual,
            vec![
                (VK_CONTROL.0, false),
                (copy_key.0, false),
                (copy_key.0, true),
                (VK_CONTROL.0, true),
            ]
        );
    }

    #[test]
    fn copy_sequence_reuses_a_physically_held_control_key() {
        let copy_key = VIRTUAL_KEY(0x43);
        let strokes = copy_key_strokes(copy_key, true, false);
        let actual: Vec<(u16, bool)> = strokes.iter().map(|s| (s.key.0, s.up)).collect();

        assert_eq!(actual, vec![(copy_key.0, false), (copy_key.0, true)]);
        assert!(!actual.iter().any(|(key, _)| *key == VK_CONTROL.0));
    }
}
