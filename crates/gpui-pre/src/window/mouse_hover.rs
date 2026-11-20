//! The hitboxes that the mouse hovers, also while a hitbox captures the pointer or a mouse
//! button is held.

use std::any::Any;

use smallvec::SmallVec;

use super::{Frame, HitTest, HitboxId};
use crate::{MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point};

#[derive(Default)]
pub(super) struct MouseHover {
    /// The hitboxes under the mouse, from the top.
    hit_test: HitTest,

    /// The hitbox that captures the pointer.
    captured: Option<HitboxId>,

    /// The held mouse buttons.
    held: SmallVec<[MouseButton; 2]>,

    /// The position of the press of the first held mouse button.
    press: Option<Point<Pixels>>,

    /// The hovered hitboxes, from the top.
    hovered: SmallVec<[HitboxId; 8]>,

    /// Whether a capture decides `hovered`.
    follows_capture: bool,
}

impl MouseHover {
    pub(super) fn hit_test(&self) -> &HitTest {
        &self.hit_test
    }

    pub(super) fn hovered(&self) -> &[HitboxId] {
        &self.hovered
    }

    /// Whether a capture decides the hovered hitboxes.
    pub(super) fn follows_capture(&self) -> bool {
        self.follows_capture
    }

    pub(super) fn captured(&self) -> Option<HitboxId> {
        self.captured
    }

    /// Sets the captured hitbox. Takes effect in the next update.
    pub(super) fn capture(&mut self, hitbox_id: Option<HitboxId>) {
        self.captured = hitbox_id;
    }

    /// Tracks the press of a mouse event at the mouse `position` before its listeners run.
    pub(super) fn begin_mouse_event(&mut self, event: &dyn Any, position: Point<Pixels>) {
        if let Some(event) = event.downcast_ref::<MouseDownEvent>() {
            if self.held.contains(&event.button) {
                // The mouse up of the earlier press was lost.
                self.held.clear();
            }
            if self.held.is_empty() {
                self.press = Some(position);
            }
            self.held.push(event.button);
        } else if let Some(event) = event.downcast_ref::<MouseMoveEvent>()
            && event.pressed_button.is_none()
        {
            // The mouse up can be lost, as in a release outside the window.
            self.held.clear();
            self.press = None;
        }
    }

    /// Applies the capture that the listeners of a mouse event ask for, and ends the press
    /// and the capture on the release of the last held button. A drag and drop ends the press,
    /// so the drop targets under the mouse are hovered. Takes effect in the next update.
    /// Returns whether the capture or the press changed.
    pub(super) fn end_mouse_event(
        &mut self,
        event: &dyn Any,
        capture: Option<HitboxId>,
        has_active_drag: bool,
    ) -> bool {
        let before = (self.captured, self.press);
        if has_active_drag {
            self.press = None;
        }
        if capture.is_some() {
            self.captured = capture;
        }
        if let Some(event) = event.downcast_ref::<MouseUpEvent>() {
            self.held.retain(|button| *button != event.button);
            if self.held.is_empty() {
                self.captured = None;
                self.press = None;
            }
        }
        (self.captured, self.press) != before
    }

    /// Finds the hovered hitboxes in `frame` with the mouse at `position`. Returns whether the
    /// look of the hover changed: the hovered hitboxes, or whether a capture decides them.
    pub(super) fn update(&mut self, frame: &Frame, position: Point<Pixels>) -> bool {
        self.hit_test = frame.hit_test(position);
        let (hovered, follows_capture) = self.find_hovered(frame, position);
        let changed = hovered != self.hovered || follows_capture != self.follows_capture;
        self.hovered = hovered;
        self.follows_capture = follows_capture;
        changed
    }

    fn find_hovered(
        &self,
        frame: &Frame,
        position: Point<Pixels>,
    ) -> (SmallVec<[HitboxId; 8]>, bool) {
        let captured_index = self.captured.and_then(|captured| {
            frame
                .hitboxes
                .iter()
                .rposition(|hitbox| hitbox.id == captured)
        });
        if let Some(captured_index) = captured_index {
            let hitboxes = &frame.hitboxes[..=captured_index];
            let captured = &hitboxes[captured_index];
            let position = captured.hit_bounds().nearest_point_inside(position);
            let hit_test = Frame::hit_test_where(hitboxes, |hitbox| {
                hitbox.id == captured.id || hitbox.hit_bounds().contains(&position)
            });
            return (SmallVec::from_slice(hit_test.hovered()), true);
        }
        let mut hovered = SmallVec::from_slice(self.hit_test.hovered());
        let Some(press) = self.press else {
            return (hovered, false);
        };
        let at_press = frame.hit_test(press);
        hovered.retain(|id| at_press.hovered().contains(id));
        (hovered, false)
    }
}
