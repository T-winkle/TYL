use super::{MouseInput, Selection};
use block2::RcBlock;
use objc2::{
    rc::{autoreleasepool, Retained},
    runtime::AnyObject,
};
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSWorkspace};
use std::{cell::RefCell, ptr::NonNull};

pub fn foreground() -> usize {
    autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| usize::try_from(app.processIdentifier()).ok())
            .unwrap_or(0)
    })
}

pub struct Monitor(Retained<AnyObject>);
impl Monitor {
    pub(super) fn install(input: MouseInput) -> Result<Self, String> {
        let input = RefCell::new(input);
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
            let event = unsafe { event.as_ref() };
            let mut input = input.borrow_mut();
            let Some(mouse_event) = event.CGEvent() else {
                return;
            };
            let pos = objc2_core_graphics::CGEvent::location(Some(&mouse_event));
            let point = Selection {
                target: foreground(),
                x: pos.x as i32,
                y: pos.y as i32,
            };
            if point.target == std::process::id() as usize {
                return;
            }
            match event.r#type() {
                NSEventType::LeftMouseDown => input.down(point),
                NSEventType::LeftMouseUp => input.up(point, event.clickCount() >= 2),
                _ => input.outside(),
            }
        });
        NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown
                | NSEventMask::LeftMouseUp
                | NSEventMask::RightMouseDown
                | NSEventMask::OtherMouseDown,
            &handler,
        )
        .map(Self)
        .ok_or_else(|| "无法监听鼠标划词 / Could not monitor mouse selections".into())
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe { NSEvent::removeMonitor(&self.0) };
    }
}
