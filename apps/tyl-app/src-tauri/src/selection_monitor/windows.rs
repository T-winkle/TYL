use super::{MouseInput, Selection};
use std::cell::RefCell;
use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::*,
};
use windows::Win32::{
    System::LibraryLoader::GetModuleHandleW, UI::Input::KeyboardAndMouse::GetDoubleClickTime,
};

pub fn foreground() -> usize {
    unsafe { GetForegroundWindow().0 as usize }
}

struct MouseHook {
    input: MouseInput,
    last: Option<(Selection, u32)>,
    double: bool,
}
thread_local! { static HOOK: RefCell<Option<MouseHook>> = const { RefCell::new(None) }; }

unsafe extern "system" fn mouse(code: i32, msg: WPARAM, param: LPARAM) -> LRESULT {
    let msg_id = msg.0 as u32;
    if code == HC_ACTION as i32
        && matches!(
            msg_id,
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_MBUTTONDOWN
        )
    {
        let data = &*(param.0 as *const MSLLHOOKSTRUCT);
        let root = GetAncestor(WindowFromPoint(data.pt), GA_ROOT);
        let mut pid = 0;
        GetWindowThreadProcessId(root, Some(&mut pid));
        HOOK.with_borrow_mut(|hook| {
            let Some(h) = hook else { return };
            if pid == std::process::id() || pid == 0 {
                h.input.start = None;
                h.last = None;
                return;
            }
            let point = Selection {
                target: root.0 as usize,
                x: data.pt.x,
                y: data.pt.y,
            };
            match msg_id {
                WM_LBUTTONDOWN => {
                    h.double = h.last.is_some_and(|(prev, time)| {
                        prev.target == point.target
                            && data.time.wrapping_sub(time) <= GetDoubleClickTime()
                            && point.x.abs_diff(prev.x)
                                <= (GetSystemMetrics(SM_CXDOUBLECLK) / 2) as u32
                            && point.y.abs_diff(prev.y)
                                <= (GetSystemMetrics(SM_CYDOUBLECLK) / 2) as u32
                    });
                    h.last = Some((point, data.time));
                    h.input.down(point);
                }
                WM_LBUTTONUP => h.input.up(point, h.double),
                _ => {
                    h.last = None;
                    h.input.outside();
                }
            }
        });
    }
    CallNextHookEx(None, code, msg, param)
}

pub struct Monitor(HHOOK);
impl Monitor {
    pub(super) fn install(input: MouseInput) -> Result<Self, String> {
        let module = unsafe { GetModuleHandleW(None) }.map_err(|e| e.to_string())?;
        let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), Some(module.into()), 0) }
            .map_err(|e| e.to_string())?;
        HOOK.with_borrow_mut(|state| {
            *state = Some(MouseHook {
                input,
                last: None,
                double: false,
            })
        });
        Ok(Self(hook))
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWindowsHookEx(self.0);
        }
        let _ = HOOK.try_with(|state| state.borrow_mut().take());
    }
}
