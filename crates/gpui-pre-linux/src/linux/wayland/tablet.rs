//! Pen input from the tablet protocol, `zwp_tablet_v2`.
//!
//! The compositor gives the events of a pen to a client that binds the
//! tablet seat as tablet tool events only, with no pointer events. So each
//! tool makes the mouse events of its window, as GTK does in
//! `tablet_tool_handle_frame` of `gdk/wayland/gdkseat-wayland.c`:
//! <https://gitlab.gnome.org/GNOME/gtk/-/blob/main/gdk/wayland/gdkseat-wayland.c>.
//! The tip is the left button. The barrel buttons are the middle and the
//! right button, in the order of GTK and Qt.
//!
//! The events of a tool come in groups that end with a `frame` event. The
//! tool applies each group at its frame.

use super::{
    ClickState, WaylandClientStatePtr,
    image_cursor::{ImageCursorSurface, ImageCursors},
    serial::{SerialKind, SerialTracker},
    window::WaylandWindowStatePtr,
};
use crate::linux::Globals;
use collections::HashMap;
use gpui::{
    CursorRequest, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent,
    PenEvent, Pixels, PlatformInput, Point, point, px,
};
use wayland_backend::{client::ObjectId, protocol::WEnum};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child, protocol::wl_seat::WlSeat,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use wayland_protocols::wp::tablet::zv2::client::{
    zwp_tablet_manager_v2::ZwpTabletManagerV2,
    zwp_tablet_pad_group_v2::{self, ZwpTabletPadGroupV2},
    zwp_tablet_pad_ring_v2::ZwpTabletPadRingV2,
    zwp_tablet_pad_strip_v2::ZwpTabletPadStripV2,
    zwp_tablet_pad_v2::{self, ZwpTabletPadV2},
    zwp_tablet_seat_v2::{self, ZwpTabletSeatV2},
    zwp_tablet_tool_v2::{self, ButtonState, ZwpTabletToolV2},
    zwp_tablet_v2::{self, ZwpTabletV2},
};

/// The barrel buttons of a pen, from `linux/input-event-codes.h`.
const BTN_STYLUS: u32 = 0x14b;
const BTN_STYLUS2: u32 = 0x14c;

/// The highest pressure of the protocol.
const MAX_PRESSURE: f32 = 65535.0;

/// The parts of the client state that the tablet events use.
pub(super) struct TabletContext<'a> {
    pub tablets: &'a mut Tablets,
    pub globals: &'a Globals,
    pub windows: &'a HashMap<ObjectId, WaylandWindowStatePtr>,
    pub serial_tracker: &'a mut SerialTracker,
    pub image_cursors: &'a mut ImageCursors,

    /// The cursor of the windows. None until a window sets one.
    pub cursor: Option<&'a CursorRequest>,
}

/// The tablet seat of the client and its tools.
pub(crate) struct Tablets {
    seat: Option<ZwpTabletSeatV2>,
    tools: HashMap<ObjectId, TabletTool>,
}

impl Tablets {
    pub(crate) fn new(
        manager: Option<&ZwpTabletManagerV2>,
        seat: &WlSeat,
        qh: &QueueHandle<WaylandClientStatePtr>,
    ) -> Self {
        Self {
            seat: manager.map(|manager| manager.get_tablet_seat(seat, qh, ())),
            tools: HashMap::default(),
        }
    }

    /// Drops the tools of the old seat and gets the tablet seat of `seat`.
    pub(crate) fn rebind(
        &mut self,
        manager: Option<&ZwpTabletManagerV2>,
        seat: &WlSeat,
        qh: &QueueHandle<WaylandClientStatePtr>,
    ) {
        for (_, tool) in self.tools.drain() {
            tool.destroy();
        }
        if let Some(seat) = self.seat.take() {
            seat.destroy();
        }
        *self = Self::new(manager, seat, qh);
    }

    /// Forgets `window` in the tools near it. The compositor sends them
    /// no `proximity_out` for a destroyed surface.
    pub(crate) fn drop_window(&mut self, window: &WaylandWindowStatePtr) {
        for tool in self.tools.values_mut() {
            if tool.window.as_ref().is_some_and(|near| near.ptr_eq(window)) {
                tool.window = None;
                tool.state = ToolState::default();
            }
        }
    }

    /// Shows `cursor` as the cursor of each tool near a window.
    pub(crate) fn set_cursor(&self, cursor: &CursorRequest, images: &mut ImageCursors) {
        for tool in self.tools.values() {
            tool.set_cursor(cursor, images);
        }
    }
}

/// A tool of the tablet seat and the window that it is near.
struct TabletTool {
    tool: ZwpTabletToolV2,
    cursor_shape_device: Option<WpCursorShapeDeviceV1>,

    /// The surface of an image cursor. A surface takes the cursor role of
    /// one device only.
    image_surface: ImageCursorSurface,
    window: Option<WaylandWindowStatePtr>,

    /// The serial of the last `proximity_in`. The cursor requests need it.
    enter_serial: u32,
    state: ToolState,
}

impl TabletTool {
    fn new(
        tool: ZwpTabletToolV2,
        globals: &Globals,
        qh: &QueueHandle<WaylandClientStatePtr>,
    ) -> Self {
        Self {
            cursor_shape_device: globals
                .cursor_shape_manager
                .as_ref()
                .map(|manager| manager.get_tablet_tool_v2(&tool, qh, ())),
            image_surface: ImageCursorSurface::new(globals),
            tool,
            window: None,
            enter_serial: 0,
            state: ToolState::default(),
        }
    }

    fn set_cursor(&self, cursor: &CursorRequest, images: &mut ImageCursors) {
        if self.window.is_none() {
            return;
        }
        self.image_surface.show_cursor(
            cursor,
            images,
            self.cursor_shape_device.as_ref(),
            self.enter_serial,
            |surface, x, y| self.tool.set_cursor(self.enter_serial, Some(surface), x, y),
        );
    }

    fn destroy(&self) {
        if let Some(device) = &self.cursor_shape_device {
            device.destroy();
        }
        self.tool.destroy();
    }
}

/// The events of a tool since its last frame.
#[derive(Debug, Default)]
struct Pending {
    proximity_in: bool,
    proximity_out: bool,
    motion: Option<Point<Pixels>>,
    pressure: Option<f32>,
    down: bool,
    up: bool,
    buttons: Vec<(MouseButton, bool)>,
}

/// What a window gets from one frame of a tool, in order.
#[derive(Debug, PartialEq)]
enum Step {
    Pen(PenEvent),

    /// The window is hovered, or no more.
    Hover(bool),
    Move {
        position: Point<Pixels>,
        pressed: Option<MouseButton>,
    },
    Press {
        button: MouseButton,
        position: Point<Pixels>,
        click_count: usize,
    },
    Release {
        button: MouseButton,
        position: Point<Pixels>,
        click_count: usize,
    },
    Exit {
        position: Point<Pixels>,
        pressed: Option<MouseButton>,
    },
}

/// The state of a tool that its frames change.
#[derive(Debug, Default)]
struct ToolState {
    near: bool,
    down: bool,
    position: Point<Pixels>,
    pressure: f32,
    pressed: Option<MouseButton>,

    /// The clicks of the tool. They do not join the clicks of the mouse.
    click: ClickState,
    pending: Pending,
}

impl ToolState {
    /// Applies the events since the last frame.
    fn frame(&mut self) -> Vec<Step> {
        let pending = std::mem::take(&mut self.pending);
        let mut steps = Vec::new();
        self.near |= pending.proximity_in;
        if !self.near {
            return steps;
        }
        if let Some(position) = pending.motion {
            self.position = position;
        }
        if let Some(pressure) = pending.pressure {
            self.pressure = pressure;
        }
        if pending.proximity_in {
            steps.push(Step::Hover(true));
        }
        let pressure_changed = self.down && pending.pressure.is_some();
        if pending.proximity_in || pending.motion.is_some() || pressure_changed {
            steps.push(Step::Pen(self.pen()));
            steps.push(Step::Move {
                position: self.position,
                pressed: self.pressed,
            });
        }
        if pending.down && !self.down {
            self.down = true;
            steps.push(Step::Pen(self.pen()));
            steps.push(self.press(MouseButton::Left));
        }
        for (button, press) in pending.buttons {
            if press {
                steps.push(self.press(button));
            } else {
                steps.push(self.release(button));
            }
        }
        if (pending.up || pending.proximity_out) && self.down {
            self.down = false;
            steps.push(Step::Pen(PenEvent::Hover));
            steps.push(self.release(MouseButton::Left));
        }
        if pending.proximity_out {
            if let Some(button) = self.pressed {
                steps.push(self.release(button));
            }
            self.near = false;
            steps.push(Step::Pen(PenEvent::Leave));
            steps.push(Step::Exit {
                position: self.position,
                pressed: None,
            });
            steps.push(Step::Hover(false));
        }
        steps
    }

    fn pen(&self) -> PenEvent {
        if self.down {
            PenEvent::Touch {
                position: self.position,
                pressure: self.pressure,
            }
        } else {
            PenEvent::Hover
        }
    }

    fn press(&mut self, button: MouseButton) -> Step {
        self.pressed = Some(button);
        Step::Press {
            button,
            position: self.position,
            click_count: self.click.press(button, self.position),
        }
    }

    fn release(&mut self, button: MouseButton) -> Step {
        if self.pressed == Some(button) {
            self.pressed = None;
        }
        Step::Release {
            button,
            position: self.position,
            click_count: self.click.count(),
        }
    }
}

impl Dispatch<ZwpTabletManagerV2, ()> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        _: &ZwpTabletManagerV2,
        _: <ZwpTabletManagerV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletSeatV2, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        _: &ZwpTabletSeatV2,
        event: zwp_tablet_seat_v2::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let zwp_tablet_seat_v2::Event::ToolAdded { id: tool } = event else {
            return;
        };
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let context = state.tablet_context();
        let tool = TabletTool::new(tool, context.globals, qh);
        context.tablets.tools.insert(tool.tool.id(), tool);
    }

    event_created_child!(WaylandClientStatePtr, ZwpTabletSeatV2, [
        zwp_tablet_seat_v2::EVT_TABLET_ADDED_OPCODE => (ZwpTabletV2, ()),
        zwp_tablet_seat_v2::EVT_TOOL_ADDED_OPCODE => (ZwpTabletToolV2, ()),
        zwp_tablet_seat_v2::EVT_PAD_ADDED_OPCODE => (ZwpTabletPadV2, ()),
    ]);
}

impl Dispatch<ZwpTabletToolV2, ()> for WaylandClientStatePtr {
    fn event(
        this: &mut Self,
        tool: &ZwpTabletToolV2,
        event: zwp_tablet_tool_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let client = this.get_client();
        let mut state = client.borrow_mut();
        let context = state.tablet_context();
        let Some(entry) = context.tablets.tools.get_mut(&tool.id()) else {
            return;
        };
        let pending = &mut entry.state.pending;
        let press_serial = match event {
            zwp_tablet_tool_v2::Event::ProximityIn {
                serial, surface, ..
            } => {
                let Some(window) = context.windows.get(&surface.id()).cloned() else {
                    return;
                };
                entry.window = Some(window);
                entry.enter_serial = serial;
                pending.proximity_in = true;
                let cursor = context.cursor.cloned().unwrap_or_default();
                entry.set_cursor(&cursor, context.image_cursors);
                None
            }
            zwp_tablet_tool_v2::Event::ProximityOut => {
                pending.proximity_out = true;
                None
            }
            zwp_tablet_tool_v2::Event::Down { serial } => {
                pending.down = true;
                Some(serial)
            }
            zwp_tablet_tool_v2::Event::Up => {
                pending.up = true;
                None
            }
            zwp_tablet_tool_v2::Event::Motion { x, y } => {
                pending.motion = Some(point(px(x as f32), px(y as f32)));
                None
            }
            zwp_tablet_tool_v2::Event::Pressure { pressure } => {
                pending.pressure = Some(pressure as f32 / MAX_PRESSURE);
                None
            }
            zwp_tablet_tool_v2::Event::Button {
                serial,
                button,
                state: WEnum::Value(button_state),
            } => {
                let button = match button {
                    BTN_STYLUS => MouseButton::Middle,
                    BTN_STYLUS2 => MouseButton::Right,
                    _ => return,
                };
                let press = button_state == ButtonState::Pressed;
                pending.buttons.push((button, press));
                press.then_some(serial)
            }
            zwp_tablet_tool_v2::Event::Frame { .. } => {
                let steps = entry.state.frame();
                let window = if entry.state.near {
                    entry.window.clone()
                } else {
                    entry.window.take()
                };
                drop(state);
                if let Some(window) = window {
                    this.dispatch_tool_steps(&window, steps);
                }
                return;
            }
            zwp_tablet_tool_v2::Event::Removed => {
                if let Some(entry) = context.tablets.tools.remove(&tool.id()) {
                    entry.destroy();
                }
                return;
            }
            _ => None,
        };
        // Requests such as popup grabs and window moves need the serial of a press.
        if let Some(serial) = press_serial {
            let surface = entry.window.as_ref().map(|window| window.surface());
            context.serial_tracker.update_with_focused_surface(
                SerialKind::MousePress,
                serial,
                surface,
            );
        }
    }
}

impl WaylandClientStatePtr {
    /// Gives `window` the pen and mouse events of one frame of a tool.
    fn dispatch_tool_steps(&self, window: &WaylandWindowStatePtr, steps: Vec<Step>) {
        let client = self.get_client();
        for step in steps {
            let modifiers = client.borrow().modifiers;
            let input = match step {
                Step::Pen(pen) => {
                    window.handle_pen(pen);
                    continue;
                }
                Step::Hover(hovered) => {
                    // The mouse keeps the window hovered when it is there too.
                    if !client.borrow().is_mouse_over(window) {
                        window.set_hovered(hovered);
                    }
                    continue;
                }
                Step::Move { position, pressed } => PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: pressed,
                    modifiers,
                }),
                Step::Press {
                    button,
                    position,
                    click_count,
                } => {
                    self.end_composition_for_press(&client, client.borrow_mut());
                    PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                        first_mouse: false,
                    })
                }
                Step::Release {
                    button,
                    position,
                    click_count,
                } => PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count,
                }),
                Step::Exit { position, pressed } => PlatformInput::MouseExited(MouseExitEvent {
                    position,
                    pressed_button: pressed,
                    modifiers,
                }),
            };
            window.handle_input(input);
        }
    }
}

/// The tablets give nothing that a window uses.
impl Dispatch<ZwpTabletV2, ()> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        tablet: &ZwpTabletV2,
        event: zwp_tablet_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_tablet_v2::Event::Removed = event {
            tablet.destroy();
        }
    }
}

/// The pads give nothing that a window uses.
impl Dispatch<ZwpTabletPadV2, ()> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        pad: &ZwpTabletPadV2,
        event: zwp_tablet_pad_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_tablet_pad_v2::Event::Removed = event {
            pad.destroy();
        }
    }

    event_created_child!(WaylandClientStatePtr, ZwpTabletPadV2, [
        zwp_tablet_pad_v2::EVT_GROUP_OPCODE => (ZwpTabletPadGroupV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadGroupV2, ()> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        _: &ZwpTabletPadGroupV2,
        _: zwp_tablet_pad_group_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }

    event_created_child!(WaylandClientStatePtr, ZwpTabletPadGroupV2, [
        zwp_tablet_pad_group_v2::EVT_RING_OPCODE => (ZwpTabletPadRingV2, ()),
        zwp_tablet_pad_group_v2::EVT_STRIP_OPCODE => (ZwpTabletPadStripV2, ()),
    ]);
}

impl Dispatch<ZwpTabletPadRingV2, ()> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        _: &ZwpTabletPadRingV2,
        _: <ZwpTabletPadRingV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpTabletPadStripV2, ()> for WaylandClientStatePtr {
    fn event(
        _: &mut Self,
        _: &ZwpTabletPadStripV2,
        _: <ZwpTabletPadStripV2 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    #[test]
    fn a_stroke_gives_pen_samples_before_its_mouse_events() {
        let mut tool = ToolState::default();
        tool.pending.motion = Some(at(5.0, 6.0));
        assert_eq!(tool.frame(), [], "a motion with no proximity");

        tool.pending.proximity_in = true;
        tool.pending.motion = Some(at(10.5, 20.25));
        tool.pending.pressure = Some(0.0);
        let hover = Step::Move {
            position: at(10.5, 20.25),
            pressed: None,
        };
        assert_eq!(
            tool.frame(),
            [Step::Hover(true), Step::Pen(PenEvent::Hover), hover]
        );

        tool.pending.down = true;
        tool.pending.pressure = Some(0.25);
        let touch = PenEvent::Touch {
            position: at(10.5, 20.25),
            pressure: 0.25,
        };
        let press = Step::Press {
            button: MouseButton::Left,
            position: at(10.5, 20.25),
            click_count: 1,
        };
        assert_eq!(tool.frame(), [Step::Pen(touch), press]);

        // A change of pressure alone is a sample too.
        tool.pending.pressure = Some(0.5);
        let touch = PenEvent::Touch {
            position: at(10.5, 20.25),
            pressure: 0.5,
        };
        let drag = Step::Move {
            position: at(10.5, 20.25),
            pressed: Some(MouseButton::Left),
        };
        assert_eq!(tool.frame(), [Step::Pen(touch), drag]);

        tool.pending.motion = Some(at(12.0, 21.0));
        tool.pending.up = true;
        let touch = PenEvent::Touch {
            position: at(12.0, 21.0),
            pressure: 0.5,
        };
        let drag = Step::Move {
            position: at(12.0, 21.0),
            pressed: Some(MouseButton::Left),
        };
        let release = Step::Release {
            button: MouseButton::Left,
            position: at(12.0, 21.0),
            click_count: 1,
        };
        assert_eq!(
            tool.frame(),
            [Step::Pen(touch), drag, Step::Pen(PenEvent::Hover), release]
        );
    }

    #[test]
    fn a_pen_that_leaves_while_it_touches_releases_its_buttons() {
        let mut tool = ToolState::default();
        tool.pending.proximity_in = true;
        tool.pending.motion = Some(at(1.0, 2.0));
        tool.pending.down = true;
        tool.pending.pressure = Some(1.0);
        tool.frame();

        tool.pending.buttons.push((MouseButton::Right, true));
        let press = Step::Press {
            button: MouseButton::Right,
            position: at(1.0, 2.0),
            click_count: 1,
        };
        assert_eq!(tool.frame(), [press]);
        assert_eq!(tool.pressed, Some(MouseButton::Right));

        tool.pending.proximity_out = true;
        let release = |button| Step::Release {
            button,
            position: at(1.0, 2.0),
            click_count: 1,
        };
        assert_eq!(
            tool.frame(),
            [
                Step::Pen(PenEvent::Hover),
                release(MouseButton::Left),
                release(MouseButton::Right),
                Step::Pen(PenEvent::Leave),
                Step::Exit {
                    position: at(1.0, 2.0),
                    pressed: None
                },
                Step::Hover(false),
            ]
        );
        assert!(!tool.near && !tool.down);
        assert_eq!(tool.pressed, None);
    }
}
