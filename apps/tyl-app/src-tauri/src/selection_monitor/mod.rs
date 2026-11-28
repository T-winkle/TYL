//! Mouse callbacks only recognize gestures. A serial worker reads the committed selection.
use crate::{pipeline::Pipeline, window_ctl, ReadyFlag};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{foreground, Monitor};
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{foreground, Monitor};
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub struct Monitor;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl Monitor {
    pub(super) fn install(_: MouseInput) -> Result<Self, String> {
        Err("Mouse selection is unsupported on this platform".into())
    }
}
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn foreground() -> usize {
    0
}

#[derive(Clone, Copy)]
pub struct Selection {
    pub target: usize,
    pub x: i32,
    pub y: i32,
}

impl Selection {
    fn completes(self, start: Self, double: bool) -> bool {
        self.target != 0
            && self.target == start.target
            && (double || self.x.abs_diff(start.x) >= 4 || self.y.abs_diff(start.y) >= 4)
    }
}

enum Event {
    Outside(u64),
    Selected(Selection, u64),
}

pub(super) struct MouseInput {
    start: Option<Selection>,
    events: mpsc::Sender<Event>,
    pipeline: Arc<Pipeline>,
}

impl MouseInput {
    fn outside(&mut self) {
        self.start = None;
        let gen = self.pipeline.invalidate();
        let _ = self.events.send(Event::Outside(gen));
    }
    fn down(&mut self, point: Selection) {
        self.outside();
        self.start = Some(point);
    }
    fn up(&mut self, point: Selection, double: bool) {
        if self
            .start
            .take()
            .is_some_and(|start| point.completes(start, double))
        {
            let _ = self
                .events
                .send(Event::Selected(point, self.pipeline.revision()));
        }
    }
}

pub fn install(app: &AppHandle, pipeline: Arc<Pipeline>) -> Result<Monitor, String> {
    let (events, rx) = mpsc::channel();
    let monitor = Monitor::install(MouseInput {
        start: None,
        events,
        pipeline: pipeline.clone(),
    })?;
    let app = app.clone();
    std::thread::Builder::new()
        .name("tyl-mouse-selection".into())
        .spawn(move || {
            let mut visible_for = None;
            loop {
                // Check for keyboard-driven app switches only while a passive popup is visible.
                let event = if let Some(target) = visible_for {
                    match rx.recv_timeout(Duration::from_millis(180)) {
                        Ok(event) => event,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if !window_ctl::passive_visible() {
                                visible_for = None;
                            } else if target != foreground() {
                                if let Some(w) = app.get_webview_window("popup") {
                                    if !w.is_focused().unwrap_or(false) {
                                        window_ctl::hide_passive(&w);
                                    }
                                }
                                visible_for = None;
                            }
                            continue;
                        }
                    }
                } else if let Ok(event) = rx.recv() {
                    event
                } else {
                    break;
                };
                match event {
                    Event::Outside(gen) => {
                        if pipeline.revision() != gen {
                            continue;
                        }
                        if let Some(w) = app.get_webview_window("popup") {
                            window_ctl::hide_passive(&w);
                        }
                        visible_for = None;
                    }
                    Event::Selected(point, gen) => {
                        if pipeline.revision() != gen {
                            continue;
                        }
                        std::thread::sleep(Duration::from_millis(75));
                        if pipeline.revision() == gen
                            && foreground() == point.target
                            && app
                                .state::<ReadyFlag>()
                                .0
                                .load(std::sync::atomic::Ordering::Acquire)
                        {
                            pipeline.on_selection(&app, gen, point);
                            visible_for = Some(point.target);
                        }
                    }
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(monitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_drag_or_double_click_in_the_same_target_finishes_a_selection() {
        let start = Selection {
            target: 1,
            x: 20,
            y: 10,
        };
        assert!(!start.completes(start, false));
        assert!(start.completes(start, true));
        assert!(Selection { x: 10, ..start }.completes(start, false));
        assert!(!Selection {
            target: 2,
            x: 10,
            ..start
        }
        .completes(start, true));
    }
}
