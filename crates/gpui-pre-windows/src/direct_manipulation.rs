use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Result;
use gpui::*;
use gpui_util::ResultExt;
use windows::Win32::{
    Foundation::*,
    Graphics::{DirectManipulation::*, Gdi::*},
    System::Com::*,
    UI::{Input::Pointer::*, WindowsAndMessaging::*},
};

use crate::*;

/// Default viewport size in pixels. The actual content size doesn't matter
/// because we're using the viewport only for gesture recognition, not for
/// visual output.
const DEFAULT_VIEWPORT_SIZE: i32 = 1000;

pub(crate) struct DirectManipulationHandler {
    manager: IDirectManipulationManager,
    update_manager: IDirectManipulationUpdateManager,
    viewport: IDirectManipulationViewport,
    _handler_cookie: u32,
    window: HWND,
    scale_factor: Rc<Cell<f32>>,
    pending_events: Rc<RefCell<Vec<PlatformInput>>>,
    updates: Rc<UpdateDemand>,
}

/// Asks the window for frames while Direct Manipulation needs an `Update` at
/// each frame: while the user touches the touchpad, while a gesture runs or
/// moves on with inertia, and until a reset of the viewport ends. The
/// viewport updates only in `Update`, and the window calls it only in a frame.
struct UpdateDemand {
    frames: Arc<WindowFrames>,
    interacting: Cell<bool>,
    in_gesture: Cell<bool>,
    resetting: Cell<bool>,
}

impl UpdateDemand {
    fn new(frames: Arc<WindowFrames>) -> Self {
        Self {
            frames,
            interacting: Cell::new(false),
            in_gesture: Cell::new(false),
            resetting: Cell::new(false),
        }
    }

    fn needs_updates(&self) -> bool {
        self.interacting.get() || self.in_gesture.get() || self.resetting.get()
    }

    fn ask_if_needed(&self) {
        if self.needs_updates() {
            self.frames.ask();
        }
    }
}

impl DirectManipulationHandler {
    pub fn new(window: HWND, scale_factor: f32, frames: Arc<WindowFrames>) -> Result<Self> {
        unsafe {
            let manager: IDirectManipulationManager =
                CoCreateInstance(&DirectManipulationManager, None, CLSCTX_INPROC_SERVER)?;

            let update_manager: IDirectManipulationUpdateManager = manager.GetUpdateManager()?;

            let viewport: IDirectManipulationViewport = manager.CreateViewport(None, window)?;

            let configuration = DIRECTMANIPULATION_CONFIGURATION_INTERACTION
                | DIRECTMANIPULATION_CONFIGURATION_TRANSLATION_X
                | DIRECTMANIPULATION_CONFIGURATION_TRANSLATION_Y
                | DIRECTMANIPULATION_CONFIGURATION_TRANSLATION_INERTIA
                | DIRECTMANIPULATION_CONFIGURATION_RAILS_X
                | DIRECTMANIPULATION_CONFIGURATION_RAILS_Y
                | DIRECTMANIPULATION_CONFIGURATION_SCALING;
            viewport.ActivateConfiguration(configuration)?;

            viewport.SetViewportOptions(
                DIRECTMANIPULATION_VIEWPORT_OPTIONS_MANUALUPDATE
                    | DIRECTMANIPULATION_VIEWPORT_OPTIONS_DISABLEPIXELSNAPPING,
            )?;

            let mut rect = RECT {
                left: 0,
                top: 0,
                right: DEFAULT_VIEWPORT_SIZE,
                bottom: DEFAULT_VIEWPORT_SIZE,
            };
            viewport.SetViewportRect(&mut rect)?;

            manager.Activate(window)?;
            viewport.Enable()?;

            let scale_factor = Rc::new(Cell::new(scale_factor));
            let pending_events = Rc::new(RefCell::new(Vec::new()));
            let updates = Rc::new(UpdateDemand::new(frames));

            let event_handler: IDirectManipulationViewportEventHandler =
                DirectManipulationEventHandler::new(
                    window,
                    Rc::clone(&scale_factor),
                    Rc::clone(&pending_events),
                    Rc::clone(&updates),
                )
                .into();

            let handler_cookie = viewport.AddEventHandler(Some(window), &event_handler)?;

            update_manager.Update(None)?;

            Ok(Self {
                manager,
                update_manager,
                viewport,
                _handler_cookie: handler_cookie,
                window,
                scale_factor,
                pending_events,
                updates,
            })
        }
    }

    pub fn set_scale_factor(&self, scale_factor: f32) {
        self.scale_factor.set(scale_factor);
    }

    pub fn on_pointer_hit_test(&self, wparam: WPARAM) {
        unsafe {
            let pointer_id = wparam.loword() as u32;
            let mut pointer_type = POINTER_INPUT_TYPE::default();
            if GetPointerType(pointer_id, &mut pointer_type).is_ok() && pointer_type == PT_TOUCHPAD
            {
                self.viewport.SetContact(pointer_id).log_err();
                // The contact takes effect at the next update.
                self.updates.frames.ask();
            }
        }
    }

    /// Updates the viewport. Asks for the next frame while the viewport
    /// needs more updates.
    pub fn update(&self) {
        unsafe {
            self.update_manager.Update(None).log_err();
        }
        self.updates.ask_if_needed();
    }

    pub fn drain_events(&self) -> Vec<PlatformInput> {
        std::mem::take(&mut *self.pending_events.borrow_mut())
    }
}

impl Drop for DirectManipulationHandler {
    fn drop(&mut self) {
        unsafe {
            self.viewport.Stop().log_err();
            self.viewport.Abandon().log_err();
            self.manager.Deactivate(self.window).log_err();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GestureKind {
    None,
    Scroll,
    Pinch,
}

#[windows_core::implement(
    IDirectManipulationViewportEventHandler,
    IDirectManipulationInteractionEventHandler
)]
struct DirectManipulationEventHandler {
    window: HWND,
    scale_factor: Rc<Cell<f32>>,
    gesture_kind: Cell<GestureKind>,
    last_scale: Cell<f32>,
    last_x_offset: Cell<f32>,
    last_y_offset: Cell<f32>,
    scroll_phase: Cell<TouchPhase>,
    pending_events: Rc<RefCell<Vec<PlatformInput>>>,
    updates: Rc<UpdateDemand>,
}

impl DirectManipulationEventHandler {
    fn new(
        window: HWND,
        scale_factor: Rc<Cell<f32>>,
        pending_events: Rc<RefCell<Vec<PlatformInput>>>,
        updates: Rc<UpdateDemand>,
    ) -> Self {
        Self {
            window,
            scale_factor,
            gesture_kind: Cell::new(GestureKind::None),
            last_scale: Cell::new(1.0),
            last_x_offset: Cell::new(0.0),
            last_y_offset: Cell::new(0.0),
            scroll_phase: Cell::new(TouchPhase::Started),
            pending_events,
            updates,
        }
    }

    /// Queues an event for the window, which takes it in its next frame.
    fn push_event(&self, event: PlatformInput) {
        self.pending_events.borrow_mut().push(event);
        self.updates.frames.ask();
    }

    fn end_gesture(&self) {
        let position = self.mouse_position();
        let modifiers = current_modifiers();
        match self.gesture_kind.get() {
            GestureKind::Scroll => {
                self.push_event(PlatformInput::Scroll(ScrollEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.0), px(0.0))),
                    modifiers,
                    touch_phase: TouchPhase::Ended,
                    source: ScrollSource::Pointer,
                }));
            }
            GestureKind::Pinch => {
                self.push_event(PlatformInput::Pinch(PinchEvent {
                    position,
                    delta: 0.0,
                    modifiers,
                    phase: TouchPhase::Ended,
                }));
            }
            GestureKind::None => {}
        }
        self.gesture_kind.set(GestureKind::None);
    }

    fn mouse_position(&self) -> Point<Pixels> {
        let scale_factor = self.scale_factor.get();
        unsafe {
            let mut point: POINT = std::mem::zeroed();
            let _ = GetCursorPos(&mut point);
            let _ = ScreenToClient(self.window, &mut point);
            logical_point(point.x as f32, point.y as f32, scale_factor)
        }
    }
}

impl IDirectManipulationViewportEventHandler_Impl for DirectManipulationEventHandler_Impl {
    fn OnViewportStatusChanged(
        &self,
        viewport: windows_core::Ref<'_, IDirectManipulationViewport>,
        current: DIRECTMANIPULATION_STATUS,
        previous: DIRECTMANIPULATION_STATUS,
    ) -> windows_core::Result<()> {
        if current == previous {
            return Ok(());
        }
        self.updates
            .in_gesture
            .set(current == DIRECTMANIPULATION_RUNNING || current == DIRECTMANIPULATION_INERTIA);

        // A new gesture interrupted inertia, so end the old sequence.
        if current == DIRECTMANIPULATION_RUNNING && previous == DIRECTMANIPULATION_INERTIA {
            self.end_gesture();
        }

        if current == DIRECTMANIPULATION_READY {
            self.end_gesture();

            // Reset the content transform so the viewport is ready for the next gesture.
            // ZoomToRect triggers a second RUNNING -> READY cycle in the updates that follow.
            // The transform is identity at that READY, so it does not reset again.
            let transformed = self.last_scale.get() != 1.0
                || self.last_x_offset.get() != 0.0
                || self.last_y_offset.get() != 0.0;
            self.last_scale.set(1.0);
            self.last_x_offset.set(0.0);
            self.last_y_offset.set(0.0);
            self.updates.resetting.set(false);
            if transformed && let Some(viewport) = viewport.as_ref() {
                // A READY inside the call ends the reset.
                self.updates.resetting.set(true);
                let reset = unsafe {
                    viewport.ZoomToRect(
                        0.0,
                        0.0,
                        DEFAULT_VIEWPORT_SIZE as f32,
                        DEFAULT_VIEWPORT_SIZE as f32,
                        false,
                    )
                };
                if reset.log_err().is_none() {
                    self.updates.resetting.set(false);
                }
            }
        }

        self.updates.ask_if_needed();
        Ok(())
    }

    fn OnViewportUpdated(
        &self,
        _viewport: windows_core::Ref<'_, IDirectManipulationViewport>,
    ) -> windows_core::Result<()> {
        Ok(())
    }

    fn OnContentUpdated(
        &self,
        _viewport: windows_core::Ref<'_, IDirectManipulationViewport>,
        content: windows_core::Ref<'_, IDirectManipulationContent>,
    ) -> windows_core::Result<()> {
        let content = content.as_ref().ok_or(E_POINTER)?;

        // Get the 6-element content transform: [scale, 0, 0, scale, tx, ty]
        let mut xform = [0.0f32; 6];
        unsafe {
            content.GetContentTransform(&mut xform)?;
        }

        let scale = xform[0];
        let scale_factor = self.scale_factor.get();
        let x_offset = xform[4] / scale_factor;
        let y_offset = xform[5] / scale_factor;

        if scale == 0.0 {
            return Ok(());
        }

        let last_scale = self.last_scale.get();
        let last_x = self.last_x_offset.get();
        let last_y = self.last_y_offset.get();

        if float_equals(scale, last_scale)
            && float_equals(x_offset, last_x)
            && float_equals(y_offset, last_y)
        {
            return Ok(());
        }

        let position = self.mouse_position();
        let modifiers = current_modifiers();

        // Direct Manipulation reports both translation and scale in every content update.
        // Translation values can shift during a pinch due to the zoom center shifting.
        // We classify each gesture as either scroll or pinch and only emit one type of event.
        // We allow Scroll -> Pinch (a pinch can start with a small pan) but not the reverse.
        if !float_equals(scale, 1.0) {
            if self.gesture_kind.get() != GestureKind::Pinch {
                self.end_gesture();
                self.gesture_kind.set(GestureKind::Pinch);
                self.push_event(PlatformInput::Pinch(PinchEvent {
                    position,
                    delta: 0.0,
                    modifiers,
                    phase: TouchPhase::Started,
                }));
            }
        } else if self.gesture_kind.get() == GestureKind::None {
            self.gesture_kind.set(GestureKind::Scroll);
            self.scroll_phase.set(TouchPhase::Started);
        }

        match self.gesture_kind.get() {
            GestureKind::Scroll => {
                let dx = x_offset - last_x;
                let dy = y_offset - last_y;
                let touch_phase = self.scroll_phase.get();
                self.scroll_phase.set(TouchPhase::Moved);
                self.push_event(PlatformInput::Scroll(ScrollEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
                    modifiers,
                    touch_phase,
                    source: ScrollSource::Pointer,
                }));
            }
            GestureKind::Pinch => {
                let scale_delta = scale / last_scale;
                self.push_event(PlatformInput::Pinch(PinchEvent {
                    position,
                    delta: scale_delta - 1.0,
                    modifiers,
                    phase: TouchPhase::Moved,
                }));
            }
            GestureKind::None => {}
        }

        self.last_scale.set(scale);
        self.last_x_offset.set(x_offset);
        self.last_y_offset.set(y_offset);

        Ok(())
    }
}

impl IDirectManipulationInteractionEventHandler_Impl for DirectManipulationEventHandler_Impl {
    fn OnInteraction(
        &self,
        _viewport: windows_core::Ref<'_, IDirectManipulationViewport2>,
        interaction: DIRECTMANIPULATION_INTERACTION_TYPE,
    ) -> windows_core::Result<()> {
        if interaction == DIRECTMANIPULATION_INTERACTION_BEGIN {
            self.updates.interacting.set(true);
        } else if interaction == DIRECTMANIPULATION_INTERACTION_END {
            self.updates.interacting.set(false);
        }
        self.updates.ask_if_needed();
        Ok(())
    }
}

fn float_equals(f1: f32, f2: f32) -> bool {
    const EPSILON_SCALE: f32 = 0.00001;
    (f1 - f2).abs() < EPSILON_SCALE * f1.abs().max(f2.abs()).max(EPSILON_SCALE)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;

    use windows::{
        Win32::{
            Foundation::HWND,
            Graphics::DirectManipulation::{
                DIRECTMANIPULATION_INERTIA, DIRECTMANIPULATION_INTERACTION_BEGIN,
                DIRECTMANIPULATION_INTERACTION_END, DIRECTMANIPULATION_INTERACTION_TYPE,
                DIRECTMANIPULATION_READY, DIRECTMANIPULATION_RUNNING, DIRECTMANIPULATION_STATUS,
                IDirectManipulationInteractionEventHandler, IDirectManipulationViewport,
                IDirectManipulationViewportEventHandler,
            },
            System::{
                Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
                LibraryLoader::GetModuleHandleW,
            },
            UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP},
        },
        core::{ComObject, w},
    };

    use super::{DirectManipulationEventHandler, DirectManipulationHandler, UpdateDemand};
    use crate::{VSyncRequests, WindowFrames};

    struct Handler {
        frames: Arc<WindowFrames>,
        updates: Rc<UpdateDemand>,
        object: ComObject<DirectManipulationEventHandler>,
    }

    impl Handler {
        fn new() -> Self {
            let requests = Arc::new(VSyncRequests::new().unwrap());
            let frames = Arc::new(WindowFrames::new(requests));
            let updates = Rc::new(UpdateDemand::new(frames.clone()));
            let object = ComObject::new(DirectManipulationEventHandler::new(
                HWND::default(),
                Rc::new(Cell::new(1.0)),
                Rc::new(RefCell::new(Vec::new())),
                updates.clone(),
            ));
            Self {
                frames,
                updates,
                object,
            }
        }

        fn status(
            &self,
            viewport: Option<&IDirectManipulationViewport>,
            current: DIRECTMANIPULATION_STATUS,
            previous: DIRECTMANIPULATION_STATUS,
        ) {
            let handler: IDirectManipulationViewportEventHandler = self.object.to_interface();
            unsafe { handler.OnViewportStatusChanged(viewport, current, previous) }.unwrap();
        }

        fn interaction(&self, interaction: DIRECTMANIPULATION_INTERACTION_TYPE) {
            let handler: IDirectManipulationInteractionEventHandler = self.object.to_interface();
            unsafe { handler.OnInteraction(None, interaction) }.unwrap();
        }
    }

    #[test]
    fn a_gesture_asks_for_frames_until_it_is_ready() {
        let handler = Handler::new();
        handler.status(None, DIRECTMANIPULATION_RUNNING, DIRECTMANIPULATION_READY);
        assert!(handler.frames.take(), "a running gesture asks for a frame");
        handler.status(None, DIRECTMANIPULATION_INERTIA, DIRECTMANIPULATION_RUNNING);
        assert!(handler.frames.take(), "inertia asks for a frame");
        handler.status(None, DIRECTMANIPULATION_READY, DIRECTMANIPULATION_INERTIA);
        assert!(!handler.updates.needs_updates());
        assert!(!handler.frames.take(), "a ready viewport asks for no frame");
    }

    #[test]
    fn an_interaction_asks_for_frames_until_it_ends() {
        let handler = Handler::new();
        handler.interaction(DIRECTMANIPULATION_INTERACTION_BEGIN);
        assert!(handler.frames.take(), "a touch asks for a frame");
        assert!(handler.updates.needs_updates());
        handler.interaction(DIRECTMANIPULATION_INTERACTION_END);
        assert!(!handler.updates.needs_updates());
        assert!(!handler.frames.take());
    }

    #[test]
    fn a_reset_asks_for_frames_until_its_ready() {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.unwrap();
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                None,
                WS_POPUP,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(GetModuleHandleW(None).unwrap().into()),
                None,
            )
        }
        .unwrap();
        let requests = Arc::new(VSyncRequests::new().unwrap());
        let manipulation =
            DirectManipulationHandler::new(window, 1.0, Arc::new(WindowFrames::new(requests)))
                .unwrap();

        let handler = Handler::new();
        handler.status(None, DIRECTMANIPULATION_RUNNING, DIRECTMANIPULATION_READY);
        handler.object.last_y_offset.set(30.0);
        assert!(handler.frames.take());

        handler.status(
            Some(&manipulation.viewport),
            DIRECTMANIPULATION_READY,
            DIRECTMANIPULATION_RUNNING,
        );
        assert!(
            handler.frames.take(),
            "the reset of a moved viewport asks for a frame"
        );
        assert!(handler.updates.needs_updates());

        handler.status(None, DIRECTMANIPULATION_RUNNING, DIRECTMANIPULATION_READY);
        handler.status(None, DIRECTMANIPULATION_READY, DIRECTMANIPULATION_RUNNING);
        assert!(
            !handler.updates.needs_updates(),
            "the READY of the reset ends it"
        );

        drop(manipulation);
        unsafe { DestroyWindow(window) }.unwrap();
    }
}
