//! UIAutomation TextPattern capture — the primary, zero-clipboard channel.
//!
//! Thread model (the part that makes or breaks this channel):
//! - A dedicated, long-lived STA thread owns the `IUIAutomation` instance.
//!   Creating `CUIAutomation` costs 10–50ms, so the instance is created once
//!   and reused for every capture.
//! - COM calls cannot be cancelled. We bound them two ways: the UIA core's
//!   own `ConnectionTimeout`/`TransactionTimeout` (IUIAutomation2, Win8+)
//!   makes providers return `UIA_E_TIMEOUT`, and the caller-side
//!   `recv_timeout` abandons the worker if it still hangs.
//! - A worker stuck past `HARD_HANG_RECOVERY` is considered eaten by a
//!   provider: we spawn a fresh STA thread and let the old one leak (never
//!   terminate a thread holding COM locks).
//! - The STA thread pumps messages between jobs, as in-proc providers may
//!   call back into our apartment.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tyl_core::locator::Rect;
use tyl_core::{CaptureAnchor, CaptureError, CaptureSource, CapturedText, TextCapture};

use super::uia_selection::{
    focused_control_editable, focused_selection, selection_editable, selection_identity,
    OwnedSafeArray,
};
use windows::core::Interface;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomation2, IUIAutomationTextRange,
};

/// Give up on the UIA channel after this long (caller-side deadline).
const UIA_SOFT_DEADLINE: Duration = Duration::from_millis(300);
/// Worker busy longer than this is presumed hung → replace the thread.
const HARD_HANG_RECOVERY: Duration = Duration::from_secs(5);

struct CaptureJob {
    /// Reserved: the UIA query targets the focused element, so the anchor
    /// currently rides along for diagnostics/future targeting only.
    #[allow(dead_code)]
    anchor: CaptureAnchor,
    reply: Sender<Result<CapturedText, CaptureError>>,
}

struct EditableProbeJob {
    reply: Sender<Option<bool>>,
}

enum Job {
    Capture(CaptureJob),
    EditableProbe(EditableProbeJob),
}

struct Worker {
    tx: Sender<Job>,
    busy_since: Arc<Mutex<Option<Instant>>>,
    /// Replaced (wedged) workers are never joined — joining a stuck STA
    /// thread would hang us too. Keeping the handle documents the leak.
    _handle: JoinHandle<()>,
}

impl Worker {
    fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let busy_since = Arc::new(Mutex::new(None));
        let thread_busy = Arc::clone(&busy_since);
        let handle = std::thread::Builder::new()
            .name("tyl-uia-sta".into())
            .spawn(move || sta_thread_main(rx, thread_busy))
            .expect("spawn UIA STA thread");
        Self {
            tx,
            busy_since,
            _handle: handle,
        }
    }

    fn healthy(&self) -> bool {
        match self.busy_since.lock().unwrap().as_ref() {
            None => true,
            Some(since) => since.elapsed() < HARD_HANG_RECOVERY,
        }
    }
}

fn sta_thread_main(rx: Receiver<Job>, busy: Arc<Mutex<Option<Instant>>>) {
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() {
            drain_and_fail(&rx, "CoInitializeEx failed on UIA thread");
            return;
        }
    }

    let uia: IUIAutomation = unsafe {
        match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
            Ok(u) => u,
            Err(_) => {
                drain_and_fail(&rx, "CUIAutomation unavailable");
                CoUninitialize();
                return;
            }
        }
    };

    // Let UIA core enforce provider timeouts so a stuck provider returns
    // UIA_E_TIMEOUT instead of blocking us forever (Win8+, harmless no-op
    // cast on older).
    if let Ok(uia2) = uia.cast::<IUIAutomation2>() {
        unsafe {
            let _ = uia2.SetConnectionTimeout(250);
            let _ = uia2.SetTransactionTimeout(2000);
        }
    }

    loop {
        *busy.lock().unwrap() = None; // idle while parked
        let job = match rx.recv() {
            Ok(j) => j,
            Err(_) => break,
        };
        *busy.lock().unwrap() = Some(Instant::now());

        match job {
            Job::Capture(CaptureJob { reply, .. }) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    capture_via_uia(&uia)
                }))
                .unwrap_or_else(|_| Err(CaptureError::Channel("UIA worker panicked".into())));
                let _ = reply.send(result);
            }
            Job::EditableProbe(EditableProbeJob { reply }) => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    focused_control_editable(&uia)
                }))
                .unwrap_or(None);
                let _ = reply.send(result);
            }
        }
        pump_messages();
    }

    unsafe { CoUninitialize() };
}

fn drain_and_fail(rx: &Receiver<Job>, msg: &str) {
    while let Ok(job) = rx.recv_timeout(Duration::from_millis(50)) {
        match job {
            Job::Capture(CaptureJob { reply, .. }) => {
                let _ = reply.send(Err(CaptureError::Channel(msg.to_string())));
            }
            Job::EditableProbe(EditableProbeJob { reply }) => {
                let _ = reply.send(None);
            }
        }
    }
}

fn pump_messages() {
    let mut msg = windows::Win32::UI::WindowsAndMessaging::MSG::default();
    unsafe {
        // Dispatch (and remove) pending thread messages; window messages we
        // don't own are left alone.
        loop {
            let removed = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &mut msg,
                None,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
            );
            if !removed.as_bool() {
                break;
            }
            if msg.hwnd.0.is_null() {
                continue;
            }
            windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
        }
    }
}

/// The actual UIA query — STA thread only.
fn capture_via_uia(uia: &IUIAutomation) -> Result<CapturedText, CaptureError> {
    let start = Instant::now();
    let selection = focused_selection(uia).map_err(CaptureError::NoText)?;
    let editable = selection_editable(uia, &selection);
    let selection_id = if editable == Some(true) {
        selection_identity(&selection)
    } else {
        None
    };
    Ok(CapturedText {
        editable,
        selection_id,
        selection_rect: text_range_first_rect(&selection.range),
        text: selection.text,
        source: CaptureSource::Uia,
        elapsed: start.elapsed(),
    })
}

/// First bounding rect of a text range, physical px, y-down.
///
/// GetBoundingRectangles returns a SAFEARRAY of R8 flats: L,T,W,H,L,T,W,H…
fn text_range_first_rect(range: &IUIAutomationTextRange) -> Option<Rect> {
    unsafe {
        let array = OwnedSafeArray(range.GetBoundingRectangles().ok()?);
        let [l, t, w, h] = first_rect_values(&array)?;
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        Some(Rect {
            left: l.round() as i32,
            top: t.round() as i32,
            right: (l + w).round() as i32,
            bottom: (t + h).round() as i32,
        })
    }
}

/// Only read the first rectangle: avoid allocating/copying every visual line.
fn first_rect_values(array: &OwnedSafeArray) -> Option<[f64; 4]> {
    use windows::Win32::System::Ole::SafeArrayGetElement;
    if !array.has_type(windows::Win32::System::Variant::VT_R8) {
        return None;
    }
    let (lo, hi) = array.bounds()?;
    if i64::from(hi) - i64::from(lo) + 1 < 4 {
        return None;
    }
    unsafe {
        let mut out = [0.0; 4];
        for (offset, value) in out.iter_mut().enumerate() {
            let index = lo.checked_add(offset as i32)?;
            SafeArrayGetElement(array.0, &index, (value as *mut f64).cast()).ok()?;
        }
        Some(out)
    }
}

/// Public capture handle; clones share one worker, replacing it if wedged.
#[derive(Clone)]
pub struct UiaCapture {
    worker: Arc<Mutex<Worker>>,
}

impl UiaCapture {
    pub fn spawn() -> Self {
        Self {
            worker: Arc::new(Mutex::new(Worker::spawn())),
        }
    }

    pub fn capture(
        &self,
        anchor: &CaptureAnchor,
        deadline: Duration,
    ) -> Result<CapturedText, CaptureError> {
        let deadline = deadline.min(UIA_SOFT_DEADLINE);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();

        {
            let mut guard = self.worker.lock().unwrap();
            if !guard.healthy() {
                *guard = Worker::spawn(); // wedged → fresh STA thread
            }
            let job = Job::Capture(CaptureJob {
                anchor: anchor.clone(),
                reply: reply_tx.clone(),
            });
            if guard.tx.send(job).is_err() {
                *guard = Worker::spawn();
                guard
                    .tx
                    .send(Job::Capture(CaptureJob {
                        anchor: anchor.clone(),
                        reply: reply_tx,
                    }))
                    .map_err(|_| CaptureError::Channel("UIA worker unreachable".into()))?;
            }
        }

        match reply_rx.recv_timeout(deadline) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                // Mark busy so `healthy()` triggers replacement after the
                // hard-hang window; report the soft miss now.
                if let Ok(guard) = self.worker.lock() {
                    let mut busy = guard.busy_since.lock().unwrap();
                    if busy.is_none() {
                        *busy = Some(Instant::now());
                    }
                }
                Err(CaptureError::NoText("UIA deadline exceeded".into()))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err(CaptureError::Channel("UIA worker died".into()))
            }
        }
    }

    /// Short, read-only capability probe used after clipboard capture. It does
    /// not attempt to read text and never turns an unknown state into writable.
    pub fn probe_focused_editable(&self, deadline: Duration) -> Option<bool> {
        let deadline = deadline.min(UIA_SOFT_DEADLINE);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        {
            let mut guard = self.worker.lock().unwrap();
            if !guard.healthy() {
                *guard = Worker::spawn();
            }
            guard
                .tx
                .send(Job::EditableProbe(EditableProbeJob { reply: reply_tx }))
                .ok()?;
        }
        reply_rx.recv_timeout(deadline).ok().flatten()
    }
}

impl TextCapture for UiaCapture {
    fn capture(
        &self,
        anchor: &CaptureAnchor,
        deadline: Duration,
    ) -> Result<CapturedText, CaptureError> {
        self.capture(anchor, deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
    use windows::Win32::System::Variant::VT_R8;

    #[test]
    fn reads_only_first_rect_and_handles_nonzero_array_bounds() {
        unsafe {
            let array = OwnedSafeArray(SafeArrayCreateVector(VT_R8, 5, 8));
            assert!(!array.0.is_null());
            for (offset, value) in [10.0f64, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0]
                .iter()
                .enumerate()
            {
                SafeArrayPutElement(array.0, &(5 + offset as i32), (value as *const f64).cast())
                    .unwrap();
            }
            assert_eq!(first_rect_values(&array), Some([10.0, 20.0, 30.0, 40.0]));
            // The owning guard releases the provider allocation on every exit path.
        }
    }

    #[test]
    fn rejects_null_and_incomplete_rectangles() {
        assert_eq!(
            first_rect_values(&OwnedSafeArray(std::ptr::null_mut())),
            None
        );
        unsafe {
            let short = OwnedSafeArray(SafeArrayCreateVector(VT_R8, 0, 3));
            assert_eq!(first_rect_values(&short), None);
            let wrong_type = OwnedSafeArray(SafeArrayCreateVector(
                windows::Win32::System::Variant::VT_I4,
                0,
                4,
            ));
            assert_eq!(first_rect_values(&wrong_type), None);
        }
    }
}
