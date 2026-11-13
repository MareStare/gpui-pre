//! Pen input from the tablet events of AppKit.
//!
//! A pen near the tablet moves the mouse. AppKit gives its samples as mouse
//! events with the tablet point subtype, and its proximity changes as
//! tablet proximity events or mouse events with the tablet proximity
//! subtype. Some tablet drivers send pure tablet point events. See
//! <https://developer.apple.com/documentation/appkit/nsevent/eventsubtype>.

use gpui::{PenEvent, Pixels, point, px};
use objc2_app_kit::{NSEvent, NSEventButtonMask, NSEventSubtype, NSEventType};

/// The pen event of an event of the view, or `None` when the event says
/// nothing about the pen. `window_height` flips the AppKit position to the
/// top-left origin of the window.
pub(crate) fn pen_event(event: &NSEvent, window_height: Pixels) -> Option<PenEvent> {
    let kind = event.r#type();
    let touches = match kind {
        NSEventType::TabletProximity => return Some(proximity(event)),
        NSEventType::TabletPoint => event.buttonMask().contains(NSEventButtonMask::PenTip),
        NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged => true,
        NSEventType::LeftMouseUp
        | NSEventType::RightMouseDown
        | NSEventType::RightMouseDragged
        | NSEventType::RightMouseUp
        | NSEventType::OtherMouseDown
        | NSEventType::OtherMouseDragged
        | NSEventType::OtherMouseUp
        | NSEventType::MouseMoved => false,
        // The pen of a drag stays: the window keeps its input.
        NSEventType::MouseExited if NSEvent::pressedMouseButtons() == 0 => {
            return Some(PenEvent::Leave);
        }
        _ => return None,
    };
    // `subtype` is valid only for mouse and tablet events.
    if kind != NSEventType::TabletPoint {
        match event.subtype() {
            NSEventSubtype::TabletPoint => {}
            NSEventSubtype::TabletProximity => return Some(proximity(event)),
            // The mouse or a trackpad moves the pointer.
            _ => return Some(PenEvent::Leave),
        }
    }
    if !touches {
        return Some(PenEvent::Hover);
    }
    let location = event.locationInWindow();
    Some(PenEvent::Touch {
        position: point(px(location.x as f32), window_height - px(location.y as f32)),
        pressure: event.pressure().clamp(0.0, 1.0),
    })
}

fn proximity(event: &NSEvent) -> PenEvent {
    if event.isEnteringProximity() {
        PenEvent::Hover
    } else {
        PenEvent::Leave
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::rc::Retained;
    use objc2_core_graphics::{CGEvent, CGEventField, CGEventType, CGMouseButton};
    use objc2_foundation::NSPoint;

    /// The values of the subtype field of a `CGEvent`.
    const SUBTYPE_MOUSE: i64 = 0;
    const SUBTYPE_TABLET_POINT: i64 = 1;
    const SUBTYPE_TABLET_PROXIMITY: i64 = 2;

    fn event(kind: CGEventType, subtype: i64, pressure: f64) -> Retained<NSEvent> {
        let point = NSPoint::new(30.25, 40.5);
        let cg_event = CGEvent::new_mouse_event(None, kind, point, CGMouseButton::Left).unwrap();
        let event = Some(&*cg_event);
        CGEvent::set_integer_value_field(event, CGEventField::MouseEventSubtype, subtype);
        CGEvent::set_double_value_field(event, CGEventField::MouseEventPressure, pressure);
        CGEvent::set_double_value_field(event, CGEventField::TabletEventPointPressure, pressure);
        let enter = CGEventField::TabletProximityEventEnterProximity;
        CGEvent::set_integer_value_field(event, enter, 1);
        NSEvent::eventWithCGEvent(&cg_event).unwrap()
    }

    fn pen(kind: CGEventType, subtype: i64) -> Option<PenEvent> {
        pen_event(&event(kind, subtype, 0.0), px(100.0))
    }

    #[test]
    fn a_tablet_point_drag_touches_and_a_mouse_drag_leaves() {
        // The event keeps the pressure in 8 bits: 51 / 255.
        let drag = event(CGEventType::LeftMouseDragged, SUBTYPE_TABLET_POINT, 0.2);
        let location = drag.locationInWindow();
        assert_eq!(
            pen_event(&drag, px(100.0)),
            Some(PenEvent::Touch {
                position: point(px(location.x as f32), px(100.0 - location.y as f32)),
                pressure: 0.2,
            })
        );
        let tablet = SUBTYPE_TABLET_POINT;
        assert_eq!(pen(CGEventType::MouseMoved, tablet), Some(PenEvent::Hover));
        assert_eq!(pen(CGEventType::LeftMouseUp, tablet), Some(PenEvent::Hover));
        let mouse = SUBTYPE_MOUSE;
        assert_eq!(
            pen(CGEventType::LeftMouseDragged, mouse),
            Some(PenEvent::Leave)
        );
        let near = SUBTYPE_TABLET_PROXIMITY;
        assert_eq!(pen(CGEventType::MouseMoved, near), Some(PenEvent::Hover));
    }
}
