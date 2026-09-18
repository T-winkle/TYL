//! Clipboard fallback channel: simulated Ctrl+C with full snapshot/restore.
//!
//! 无痕协议（每一步都对应真机上翻过车的地方，注释里有出处）：
//!
//! 1. 还欠账 —— 上次还原失败时剪贴板残留的是我们的污染物，必须先
//!    补还原，否则会把污染物当用户内容快照（滚雪球 bug）。
//! 2. 等输入静默 —— 鼠标键释放后选区才确定；热键紧跟松手时注入的
//!    Ctrl+C 会与选择余波交错（WPS 两轮写剪贴板）。
//! 3. 等冲突修饰键释放 —— 不在 Alt/Shift/Win 仍被按住时注入任何按键，
//!    避免 Edge 等应用切到菜单或清掉原选区；Ctrl 可直接复用。
//! 4. 快照 —— 在等待完成后立即用 OleGetClipboard 全格式深拷贝，避免
//!    用等待前的旧快照覆盖期间发生的合法剪贴板写入。
//! 5. 模拟 Ctrl+C —— 冲突修饰键已经释放，发送唯一一次干净的 Ctrl+C。
//! 6. 读文本 + 打隐身标记 —— "Clipboard Viewer Ignore" 让 Ditto/Win+V
//!    不记录我们取的词。
//! 7. 还原 —— 内容感知守卫（剪贴板内容仍=本次取词文本才算我们的事）
//!    + 回读验证。失败记欠账，下次先还。

use std::time::{Duration, Instant};

use tyl_core::{CaptureAnchor, CaptureError, CaptureSource, CapturedText, TextCapture};

use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
use windows::Win32::System::Com::{
    CoInitializeEx, IDataObject, COINIT_APARTMENTTHREADED, FORMATETC, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{OleGetClipboard, ReleaseStgMedium, CF_UNICODETEXT};
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
}

pub struct ClipboardFallback {
    /// When false (CLI `--no-restore` / user setting), skip the restore step.
    pub restore: bool,
    /// 还原失败的"欠账"：上一次没还回去的用户快照。存在期间剪贴板
    /// 残留的是我们的污染物，下次取词必须先还账（见模块注释步骤 1）。
    debt: std::sync::Mutex<Option<ClipboardSnapshot>>,
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

        // ── 1. 还欠账（上次还原失败？）─────────────────────────────
        {
            let mut debt = self.debt.lock().unwrap();
            if let Some(owed) = debt.take() {
                if restore_snapshot(&owed) {
                    log_str("[clip] 欠账补还原成功");
                } else {
                    log_str("[clip] 欠账仍无法还原，放弃本次取词（防污染滚雪球）");
                    *debt = Some(owed);
                    return Err(CaptureError::Channel(
                        "上一次取词的剪贴板还原仍未完成，已跳过本次（避免叠加污染）".into(),
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
        let snapshot = snapshot_via_ole()
            .map_err(|e| CaptureError::Channel(format!("clipboard snapshot failed: {e}")))?;
        log_str(&format!(
            "[clip] 快照: {} 个格式（不记录剪贴板内容）",
            snapshot.entries.len()
        ));
        let seq0 = unsafe { GetClipboardSequenceNumber() };

        // ── 5. 模拟唯一一次干净的 Ctrl+C ──────────────────────────
        if !send_ctrl_c() {
            return Err(CaptureError::Channel(
                "failed to inject the copy shortcut".into(),
            ));
        }

        // ── 6. 读文本 + 隐身标记 ───────────────────────────────────
        let mut captured = String::new();
        // 松键等待不占用目标应用响应预算；复制完成后仍给它完整的正常
        // 预算，以覆盖 WPS 等应用的多轮剪贴板写入。
        let response_deadline = Instant::now() + capture_budget.max(Duration::from_millis(500));
        loop {
            std::thread::sleep(POLL_INTERVAL);
            if unsafe { GetClipboardSequenceNumber() } != seq0 {
                if let Some(text) = read_clipboard_text() {
                    if !text.trim().is_empty() {
                        captured = text;
                        break;
                    }
                }
            }

            if Instant::now() >= response_deadline {
                break;
            }
        }

        if captured.trim().is_empty() {
            log_str("[clip] 模拟复制后剪贴板无文本（选区未提交？）");
            // seq 动过说明目标写过（可能清了又没写回）→ 剪贴板可能已被
            // 破坏，还原快照再报错，不留烂摊子。
            if unsafe { GetClipboardSequenceNumber() } != seq0 {
                let _ = restore_snapshot(&snapshot);
                log_str("[clip] seq 已变化，已还原快照后退回错误");
            }
            return Err(CaptureError::NoText(
                "clipboard channel produced no text".into(),
            ));
        }

        // 隐身标记本身会 bump seq，所以之后不能再用 seq 判断"谁动过
        // 剪贴板"——还原守卫改用内容感知（见步骤 7）。
        mark_clipboard_ignore();

        // ── 7. 还原 ────────────────────────────────────────────────
        let restored = self.restore && {
            // 内容感知守卫：剪贴板当前文本仍 == 本次取词文本，说明期间
            // 的额外写入都是目标应用补完自己的复制（我们的连锁，非第三
            // 方），还原不会覆盖任何人的数据。内容变了 = 真第三方写入，
            // 放弃还原并如实上报。
            let still_ours = read_clipboard_text()
                .map(|t| t == captured)
                .unwrap_or(false);
            if still_ours {
                restore_snapshot(&snapshot)
            } else {
                log_str("[clip] 守卫拒绝还原：内容已非本次取词文本（第三方写入？）");
                false
            }
        };

        log_str(&format!(
            "[clip] 取词 {}ms 还原={restored} 字符数={}",
            start.elapsed().as_millis(),
            captured.chars().count()
        ));

        // 还原失败 → 记欠账（下次取词步骤 1 补还原）。
        if self.restore && !restored {
            *self.debt.lock().unwrap() = Some(snapshot);
            log_str("[clip] 已记录欠账，下次取词前补还原");
        }

        Ok(CapturedText {
            text: captured,
            editable: None,
            selection_id: None,
            source: CaptureSource::Clipboard { restored },
            selection_rect: None, // no geometry from this channel
            elapsed: start.elapsed(),
        })
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
            let _ = restore_if_owned(&snapshot, self.owner, self.sequence, &self.expected);
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
        let restored = restore_if_owned(&snapshot, self.owner, self.sequence, &self.expected);
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
    let snapshot = snapshot_via_ole().map_err(|_| "无法备份剪贴板，请重试")?;
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
    let initial_sequence = write_hidden_text(owner, &probe)?;
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
        if let Some(text) = read_clipboard_text() {
            if !clipboard_text_matches(expected, &text) {
                log_str("[replace] 复制结果与原选区不同，停止替换");
                return Err("原选区已变化，请重新划词".into());
            }
            if !matched || guard.sequence != sequence {
                // Mark immediately rather than waiting out the quiet period.
                mark_clipboard_ignore();
                guard.expected = text;
                guard.sequence = unsafe { GetClipboardSequenceNumber() };
                last_sequence = guard.sequence;
                matched = true;
            }
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
    let snapshot = match verification
        .as_mut()
        .and_then(|guard| guard.snapshot.take())
    {
        Some(snapshot) => snapshot,
        None => snapshot_via_ole().map_err(|_| "无法备份剪贴板，请重试")?,
    };

    let sequence = match write_hidden_text(owner, text) {
        Ok(sequence) => sequence,
        Err(error) => {
            let _ = restore_snapshot(&snapshot);
            return Err(error);
        }
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

/// 在当前剪贴板上追加 "Clipboard Viewer Ignore" 约定格式（DWORD 0）。
/// 配合的剪贴板管理器（Ditto、Win+V 历史）会跳过这次变化、不记历史
/// ——KeePass 等密码管理器同款手法。还原时快照里没有此格式，
/// EmptyClipboard 后自然消失。失败只影响 Ditto 显示，静默降级。
fn mark_clipboard_ignore() {
    unsafe {
        let wide: Vec<u16> = "Clipboard Viewer Ignore"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let format = RegisterClipboardFormatW(windows::core::PCWSTR(wide.as_ptr()));
        if format == 0 {
            return;
        }
        for _ in 0..6 {
            if OpenClipboard(None).is_ok() {
                if let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, 4) {
                    let ptr = GlobalLock(handle) as *mut u32;
                    if !ptr.is_null() {
                        *ptr = 0u32;
                        let _ = GlobalUnlock(handle);
                        let _ = SetClipboardData(
                            format,
                            Some(windows::Win32::Foundation::HANDLE(handle.0)),
                        );
                        log_str("[clip] 已标记 Clipboard Viewer Ignore（对 Ditto/Win+V 隐身）");
                    } else {
                        let _ = GlobalFree(Some(handle));
                    }
                }
                let _ = CloseClipboard();
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        log_str("[clip] 标记 ignore 失败（剪贴板被占用），Ditto 可能仍会记录");
    }
}

fn clipboard_ignore_format() -> Option<u32> {
    let wide: Vec<u16> = "Clipboard Viewer Ignore"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let format = unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(wide.as_ptr())) };
    (format != 0).then_some(format)
}

fn write_hidden_text(owner: usize, text: &str) -> Result<u32, String> {
    let format = clipboard_ignore_format().ok_or("无法启用无痕剪贴板，请重试")?;
    let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    let text_handle = bytes_to_hglobal(&bytes).ok_or("无法分配剪贴板内存")?;
    let ignore_handle = match bytes_to_hglobal(&0u32.to_le_bytes()) {
        Some(handle) => handle,
        None => {
            unsafe {
                let _ = GlobalFree(Some(text_handle));
            }
            return Err("无法分配无痕剪贴板标记".into());
        }
    };

    let mut opened = false;
    for _ in 0..15 {
        if unsafe { OpenClipboard(Some(windows::Win32::Foundation::HWND(owner as *mut _))) }.is_ok()
        {
            opened = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !opened {
        unsafe {
            let _ = GlobalFree(Some(text_handle));
            let _ = GlobalFree(Some(ignore_handle));
        }
        return Err("剪贴板正被占用，请重试".into());
    }

    let mut ignore_transferred = false;
    let mut text_transferred = false;
    let result = unsafe {
        (|| {
            EmptyClipboard().map_err(|_| "无法清空剪贴板")?;
            SetClipboardData(
                format,
                Some(windows::Win32::Foundation::HANDLE(ignore_handle.0)),
            )
            .map_err(|_| "无法写入无痕剪贴板标记")?;
            ignore_transferred = true;
            SetClipboardData(
                CLIPBOARD_FORMAT,
                Some(windows::Win32::Foundation::HANDLE(text_handle.0)),
            )
            .map_err(|_| "无法写入替换译文")?;
            text_transferred = true;
            Ok::<(), &str>(())
        })()
    };
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let _ = unsafe { CloseClipboard() };
    if !ignore_transferred {
        unsafe {
            let _ = GlobalFree(Some(ignore_handle));
        }
    }
    if !text_transferred {
        unsafe {
            let _ = GlobalFree(Some(text_handle));
        }
    }
    result.map_err(|error| format!("{error}，请重试"))?;
    log_str("[replace] 译文与 Clipboard Viewer Ignore 已原子写入");
    Ok(sequence)
}

/// Check ownership and restore under the same clipboard lock so a third-party
/// copy cannot land between the check and EmptyClipboard.
fn restore_if_owned(
    snapshot: &ClipboardSnapshot,
    owner: usize,
    sequence: u32,
    expected: &str,
) -> bool {
    for _ in 0..15 {
        unsafe {
            if OpenClipboard(Some(windows::Win32::Foundation::HWND(owner as *mut _))).is_ok() {
                let current = GetClipboardData(CLIPBOARD_FORMAT)
                    .ok()
                    .and_then(|handle| hglobal_bytes(HGLOBAL(handle.0)))
                    .and_then(|bytes| utf16_bytes_to_string(&bytes));
                let owned = GetClipboardSequenceNumber() == sequence
                    && current.as_deref() == Some(expected);
                let restored = owned && restore_locked(snapshot);
                let _ = CloseClipboard();
                return restored;
            }
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    false
}

// ---------------------------------------------------------------------------
// 读 / 快照 / 还原
// ---------------------------------------------------------------------------

fn read_clipboard_text() -> Option<String> {
    unsafe {
        IsClipboardFormatAvailable(CLIPBOARD_FORMAT).ok()?;
        OpenClipboard(None).ok()?;
        let text = (|| {
            let handle = GetClipboardData(CLIPBOARD_FORMAT).ok()?;
            utf16_bytes_to_string(&hglobal_bytes(HGLOBAL(handle.0))?)
        })();
        let _ = CloseClipboard();
        text
    }
}

fn snapshot_via_ole() -> Result<ClipboardSnapshot, String> {
    unsafe {
        let data: IDataObject = OleGetClipboard().map_err(|e| format!("OleGetClipboard: {e}"))?;
        let enum_fmt = data
            .EnumFormatEtc(1 /* DATADIR_GET */)
            .map_err(|e| format!("EnumFormatEtc: {e}"))?;

        let mut entries = Vec::new();
        let mut batch = [FORMATETC::default(); 8];
        loop {
            let mut fetched = 0u32;
            if enum_fmt.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            for fe in &batch[..fetched as usize] {
                // 请求每个格式的 HGLOBAL 渲染；GetData 失败（延迟渲染
                // 等）→ 跳过该格式。
                let mut request = *fe;
                request.tymed = TYMED_HGLOBAL.0 as u32;
                if let Ok(medium) = data.GetData(&request) {
                    if medium.tymed == TYMED_HGLOBAL.0 as u32 {
                        if let Some(bytes) = hglobal_bytes(HGLOBAL(medium.u.hGlobal.0)) {
                            entries.push(SnapshotEntry {
                                format: request.cfFormat as u32,
                                bytes,
                            });
                        }
                    }
                    let mut m = medium;
                    ReleaseStgMedium(&mut m);
                }
            }
            if fetched < batch.len() as u32 {
                break;
            }
        }
        Ok(ClipboardSnapshot { entries })
    }
}

/// 还原快照。返回值 = **回读验证后**剪贴板确实恢复为快照内容，
/// 不是"API 调用没报错"——徽章显示的就是这个值，必须可信。
fn restore_snapshot(snapshot: &ClipboardSnapshot) -> bool {
    // OpenClipboard 被占用是常态（剪贴板管理器、目标应用都在碰），
    // 带 ~120ms 重试。
    for _ in 0..12 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            let wrote_all = unsafe { restore_locked(snapshot) };
            let _ = unsafe { CloseClipboard() };
            return wrote_all && verify_restored(snapshot);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
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
        match bytes_to_hglobal(&entry.bytes) {
            Some(handle) => {
                if SetClipboardData(
                    entry.format,
                    Some(windows::Win32::Foundation::HANDLE(handle.0)),
                )
                .is_err()
                {
                    let _ = GlobalFree(Some(handle)); // 失败时句柄仍归我们
                    ok = false;
                }
            }
            None => ok = false,
        }
    }
    ok
}

/// 回读验证：当前剪贴板文本 == 快照的 CF_UNICODETEXT。
/// 快照没有文本格式（纯图片等）时以格式计数为准（restore_locked 已校验）。
fn verify_restored(snapshot: &ClipboardSnapshot) -> bool {
    match snapshot
        .entries
        .iter()
        .find(|e| e.format == CLIPBOARD_FORMAT)
        .and_then(|e| utf16_bytes_to_string(&e.bytes))
    {
        Some(expected) => read_clipboard_text()
            .map(|t| t == expected)
            .unwrap_or(false),
        None => true,
    }
}

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// UTF-16 LE 字节 → String（去尾部 NUL）。奇数长度返回 None。
fn utf16_bytes_to_string(bytes: &[u8]) -> Option<String> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let (chunks, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = chunks.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let mut len = units.len();
    while len > 0 && units[len - 1] == 0 {
        len -= 1;
    }
    Some(String::from_utf16_lossy(&units[..len]))
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
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).ok()?;
        let ptr = GlobalLock(handle) as *mut u8;
        if ptr.is_null() {
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
