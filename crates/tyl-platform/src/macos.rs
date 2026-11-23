//! Read selected text through Accessibility without sending keys or using the pasteboard.

use std::ptr::{self, NonNull};
use std::time::{Duration, Instant};

use objc2::rc::autoreleasepool;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_application_services::{
    kAXTrustedCheckOptionPrompt, AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions,
    AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    CFBoolean, CFDictionary, CFRetained, CFString, CFType, CGRect, ConcreteType,
};
use objc2_core_graphics::CGEvent;
use tyl_core::capture::CaptureOutcome;
use tyl_core::config::FallbackPolicy;
use tyl_core::locator::{Point, Rect};
use tyl_core::{
    CaptureAnchor, CaptureError, CaptureSource, CapturedText, MonitorInfo, ScreenLocator,
    TextCapture,
};

pub struct MacCapture {
    policy: FallbackPolicy,
}

pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn request_accessibility() {
    unsafe {
        let options =
            CFDictionary::from_slices(&[kAXTrustedCheckOptionPrompt], &[CFBoolean::new(true)]);
        AXIsProcessTrustedWithOptions(Some(options.as_opaque()));
    }
}

impl MacCapture {
    pub fn with_policy(policy: FallbackPolicy) -> Self {
        Self { policy }
    }

    pub fn with_policy_and_restore(policy: FallbackPolicy, _restore: bool) -> Self {
        Self::with_policy(policy)
    }

    pub fn capture_detailed(&self, anchor: &CaptureAnchor, deadline: Duration) -> CaptureOutcome {
        match self.capture(anchor, deadline) {
            Ok(captured) => CaptureOutcome::Primary(captured),
            Err(error) => CaptureOutcome::Failed(error),
        }
    }
}

impl TextCapture for MacCapture {
    fn capture(
        &self,
        _anchor: &CaptureAnchor,
        deadline: Duration,
    ) -> Result<CapturedText, CaptureError> {
        if matches!(self.policy, FallbackPolicy::ClipboardOnly) {
            return Err(CaptureError::FallbackDisabled("macOS".into()));
        }
        if !accessibility_trusted() {
            return Err(CaptureError::PermissionDenied);
        }
        if deadline.is_zero() {
            return Err(CaptureError::NoText("capture deadline elapsed".into()));
        }

        let started = Instant::now();
        let system = unsafe { AXUIElement::new_system_wide() };
        // The system-wide timeout applies to each of the five possible AX calls below.
        let status = unsafe { system.set_messaging_timeout(deadline.as_secs_f32() / 5.0) };
        if status != AXError::Success {
            return Err(CaptureError::Channel(format!(
                "AX timeout setup failed ({})",
                status.0
            )));
        }
        let focused: CFRetained<AXUIElement> = attribute(&system, "AXFocusedUIElement")?;
        if attribute::<CFString>(&focused, "AXSubrole")
            .is_ok_and(|role| *role == *CFString::from_static_str("AXSecureTextField"))
        {
            return Err(CaptureError::NoText("secure text field".into()));
        }
        let text = attribute::<CFString>(&focused, "AXSelectedText")?.to_string();
        if text.trim().is_empty() {
            return Err(CaptureError::NoText("empty selection".into()));
        }
        Ok(CapturedText {
            text,
            editable: None,
            selection_id: None,
            source: CaptureSource::Uia,
            selection_rect: selection_bounds(&focused),
            elapsed: started.elapsed(),
        })
    }
}

fn attribute<T: ConcreteType>(
    element: &AXUIElement,
    name: &'static str,
) -> Result<CFRetained<T>, CaptureError> {
    let mut value = ptr::null();
    let status = unsafe {
        element.copy_attribute_value(&CFString::from_static_str(name), NonNull::from(&mut value))
    };
    match status {
        AXError::Success => retained_value(value).ok_or_else(|| CaptureError::NoText(name.into())),
        AXError::NoValue | AXError::AttributeUnsupported => Err(CaptureError::NoText(name.into())),
        _ => Err(CaptureError::Channel(format!(
            "{name} failed ({})",
            status.0
        ))),
    }
}

fn selection_bounds(element: &AXUIElement) -> Option<Rect> {
    let range: CFRetained<AXValue> = attribute(element, "AXSelectedTextRange").ok()?;
    if unsafe { range.r#type() } != AXValueType::CFRange {
        return None;
    }
    let mut value = ptr::null();
    let status = unsafe {
        element.copy_parameterized_attribute_value(
            &CFString::from_static_str("AXBoundsForRange"),
            &range,
            NonNull::from(&mut value),
        )
    };
    if status != AXError::Success {
        return None;
    }
    let bounds: CFRetained<AXValue> = retained_value(value)?;
    let mut rect = CGRect::default();
    if unsafe { bounds.value(AXValueType::CGRect, NonNull::from(&mut rect).cast()) }
        && !rect.is_empty()
    {
        Some(top_left_rect(rect))
    } else {
        None
    }
}

fn retained_value<T: ConcreteType>(value: *const CFType) -> Option<CFRetained<T>> {
    // Both AX copy APIs transfer ownership of the returned CF object on success.
    let value = NonNull::new(value.cast_mut())?;
    unsafe { CFRetained::from_raw(value) }.downcast().ok()
}

pub fn capture_anchor() -> CaptureAnchor {
    autoreleasepool(|_| {
        let cursor = CGEvent::new(None).map(|event| {
            let point = CGEvent::location(Some(&event));
            Point {
                x: point.x.round() as i32,
                y: point.y.round() as i32,
            }
        });
        let target_exe = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.bundleIdentifier())
            .map(|identifier| identifier.to_string().to_ascii_lowercase());
        CaptureAnchor { cursor, target_exe }
    })
}

#[derive(Default)]
pub struct MacLocator;

impl ScreenLocator for MacLocator {
    fn monitors(&self) -> Vec<MonitorInfo> {
        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };
        autoreleasepool(|_| {
            let screens = NSScreen::screens(mtm);
            let Some(primary) = screens.firstObject() else {
                return Vec::new();
            };
            let desktop_top = primary.frame().size.height;
            // AX and Quartz use a top-left origin. AppKit screen frames use bottom-left.
            let convert = |mut rect: CGRect| {
                rect.origin.y = desktop_top - rect.origin.y - rect.size.height;
                top_left_rect(rect)
            };
            // Global points keep adjacent displays contiguous even with mixed backing scales.
            screens
                .iter()
                .map(|screen| MonitorInfo {
                    rect: convert(screen.frame()),
                    work: convert(screen.visibleFrame()),
                    scale: 1.0,
                })
                .collect()
        })
    }
}

fn top_left_rect(rect: CGRect) -> Rect {
    Rect {
        left: rect.origin.x.round() as i32,
        top: rect.origin.y.round() as i32,
        right: (rect.origin.x + rect.size.width).round() as i32,
        bottom: (rect.origin.y + rect.size.height).round() as i32,
    }
}
