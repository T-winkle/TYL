//! Popup placement geometry.
//!
//! All coordinates are **physical pixels**. Convert to logical at the
//! windowing boundary (Tauri `set_position` takes logical units) — see
//! `scale` helpers at the bottom of this file.

use serde::{Deserialize, Serialize};

/// Physical-pixel point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Physical-pixel rectangle. Windows convention: top-left origin, y grows
/// downward (matches both Win32 screen coords and UIA BoundingRectangles).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

impl Size {
    pub fn new(width: i32, height: i32) -> Self {
        Self { width, height }
    }
}

/// A monitor's total and working (taskbar-excluded) rectangles, physical px.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MonitorInfo {
    pub rect: Rect,
    pub work: Rect,
    pub scale: f64,
}

impl MonitorInfo {
    pub fn work_size(&self) -> Size {
        Size::new(
            self.work.right - self.work.left,
            self.work.bottom - self.work.top,
        )
    }
}

/// Pure placement policy shared by every platform:
/// anchor below the selection (or at the cursor), flip vertically when it
/// would overflow the monitor's work area, clamp horizontally.
///
/// `selection` wins over `anchor` when present; pass the cursor position as
/// `anchor` and the selection rect optionally.
pub fn place_popup(
    selection: Option<Rect>,
    cursor: Option<Point>,
    popup: Size,
    monitor: &MonitorInfo,
    margin: i32,
) -> Point {
    // Preferred anchor: just below the selection's left edge; else cursor
    // offset down-right so the popup doesn't sit under the pointer.
    let (mut x, mut y, flip_reference_top) = match (selection, cursor) {
        (Some(r), _) => (r.left, r.bottom + margin, r.top),
        (None, Some(c)) => (c.x + margin, c.y + 2 * margin, c.y),
        (None, None) => (
            monitor.work.left + margin,
            monitor.work.top + margin,
            monitor.work.top,
        ),
    };

    let work = monitor.work;
    let w = popup.width.max(0);
    let h = popup.height.max(0);

    // Vertical: flip above the selection when overflowing the work area.
    if y + h > work.bottom - margin {
        y = flip_reference_top - h - margin;
    }
    // Clamp into the work area, margin-aware (never overlap the edge).
    y = y.clamp(
        work.top + margin,
        (work.bottom - h - margin).max(work.top + margin),
    );

    // Horizontal clamp, margin-aware.
    x = x.clamp(
        work.left + margin,
        (work.right - w - margin).max(work.left + margin),
    );

    Point { x, y }
}

/// Physical → logical (per-monitor DPI).
pub fn to_logical(v: i32, scale: f64) -> f64 {
    v as f64 / scale.max(0.1)
}

/// Logical → physical (per-monitor DPI).
pub fn to_physical(v: f64, scale: f64) -> i32 {
    (v * scale.max(0.1)).round() as i32
}

/// Picks the monitor whose rect contains the point; falls back to the
/// closest one by center distance. Pure so it can be unit-tested.
pub fn pick_monitor(monitors: &[MonitorInfo], p: Point) -> &MonitorInfo {
    debug_assert!(!monitors.is_empty(), "at least one monitor required");
    for m in monitors {
        if p.x >= m.rect.left && p.x < m.rect.right && p.y >= m.rect.top && p.y < m.rect.bottom {
            return m;
        }
    }
    // Not contained: closest center wins (handles points slightly off-edge).
    monitors
        .iter()
        .min_by_key(|m| {
            let cx = (m.rect.left + m.rect.right) / 2;
            let cy = (m.rect.top + m.rect.bottom) / 2;
            let dx = p.x - cx;
            let dy = p.y - cy;
            dx * dx + dy * dy
        })
        .expect("non-empty monitors")
}

/// Platform backend contract: enumerate monitors and locate a point's monitor.
pub trait ScreenLocator: Send + Sync {
    fn monitors(&self) -> Vec<MonitorInfo>;
    fn monitor_at(&self, p: Point) -> MonitorInfo {
        let monitors = self.monitors();
        *pick_monitor(&monitors, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono() -> MonitorInfo {
        MonitorInfo {
            rect: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            work: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            },
            scale: 1.0,
        }
    }

    fn size() -> Size {
        Size::new(400, 300)
    }

    #[test]
    fn anchor_below_selection() {
        let r = Rect {
            left: 100,
            top: 200,
            right: 500,
            bottom: 220,
        };
        let p = place_popup(Some(r), None, size(), &mono(), 8);
        assert_eq!(p, Point { x: 100, y: 228 }); // bottom + margin
    }

    #[test]
    fn flips_above_when_overflow() {
        // Selection near the bottom of the work area → popup above it.
        let r = Rect {
            left: 100,
            top: 900,
            right: 500,
            bottom: 920,
        };
        let p = place_popup(Some(r), None, size(), &mono(), 8);
        assert_eq!(p.y, 900 - 300 - 8);
    }

    #[test]
    fn clamps_left_and_right() {
        let r = Rect {
            left: 5,
            top: 200,
            right: 10,
            bottom: 220,
        };
        let p = place_popup(Some(r), None, size(), &mono(), 8);
        assert_eq!(p.x, 8); // work.left + margin

        let r = Rect {
            left: 1900,
            top: 200,
            right: 1910,
            bottom: 220,
        };
        let p = place_popup(Some(r), None, size(), &mono(), 8);
        assert_eq!(p.x, 1920 - 400 - 8);
    }

    #[test]
    fn cursor_anchor_when_no_rect() {
        let p = place_popup(None, Some(Point { x: 960, y: 540 }), size(), &mono(), 8);
        assert_eq!(p, Point { x: 968, y: 556 });
    }

    #[test]
    fn never_crosses_monitor() {
        // Selection on a right-side monitor, popup clamped into that monitor.
        let right = MonitorInfo {
            rect: Rect {
                left: 1920,
                top: 0,
                right: 3840,
                bottom: 1080,
            },
            work: Rect {
                left: 1920,
                top: 0,
                right: 3840,
                bottom: 1040,
            },
            scale: 1.0,
        };
        // Selection's left edge would place popup partially on left monitor:
        let r = Rect {
            left: 1930,
            top: 200,
            right: 2400,
            bottom: 220,
        };
        let p = place_popup(Some(r), None, size(), &right, 8);
        assert_eq!(p.x, 1930); // fully inside right monitor
                               // A selection straddling the boundary still stays inside:
        let r = Rect {
            left: 3800,
            top: 200,
            right: 3900,
            bottom: 220,
        };
        let p = place_popup(Some(r), None, size(), &right, 8);
        assert_eq!(p.x, 3840 - 400 - 8);
    }

    #[test]
    fn picks_containing_and_closest_monitor() {
        let monitors = [
            mono(),
            MonitorInfo {
                rect: Rect {
                    left: 1920,
                    top: 0,
                    right: 3840,
                    bottom: 1080,
                },
                work: Rect {
                    left: 1920,
                    top: 0,
                    right: 3840,
                    bottom: 1040,
                },
                scale: 1.25,
            },
        ];
        assert_eq!(pick_monitor(&monitors, Point { x: 500, y: 500 }).scale, 1.0);
        assert_eq!(
            pick_monitor(&monitors, Point { x: 2000, y: 100 }).scale,
            1.25
        );
        // Off-screen point → nearest center.
        assert_eq!(
            pick_monitor(&monitors, Point { x: -500, y: 500 }).scale,
            1.0
        );
    }

    #[test]
    fn scale_roundtrip() {
        assert_eq!(to_physical(to_logical(1234, 1.25), 1.25), 1234);
        assert_eq!(to_logical(250, 1.25), 200.0);
    }
}
