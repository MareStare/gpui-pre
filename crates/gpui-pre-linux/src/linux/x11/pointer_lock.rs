//! A pointer lock holds the cursor at a point of a window and moves a virtual
//! pointer by the relative motion of the mouse.
//!
//! An active pointer grab confines the cursor to a 1x1 input-only child window
//! at the point. Raw motion of XInput 2 on the root window gives the motion,
//! because raw events come before the server clamps the cursor. The server
//! can send motion events of the window before raw motion starts, such as
//! the motion of a drag right after its press. Such an event gives the
//! position of the cursor, because the cursor is free until the grab.
//!
//! The server maps the range of each axis of an absolute device, such as a
//! pen or the tablet of a virtual machine, to the whole screen. The raw values of the device hold its
//! coordinate transformation matrix. So the motion of a pen is the change of
//! its values in that scale.

use std::collections::BTreeMap;

use anyhow::{Context as _, ensure};
use gpui::{MouseButton, Pixels, Point, point, px};
use gpui_util::ResultExt as _;
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE,
    connection::Connection as _,
    protocol::{
        xinput::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
    xcb_ffi::XCBConnection,
};

use super::{
    XINPUT_ALL_DEVICE_GROUPS, check_reply, fp3232_to_f32, get_valuator_axis_index,
    pen::{AxisRange, PointerAction},
    pressed_button_from_mask,
};

#[derive(Debug)]
pub(super) struct PointerLock {
    pub window: xproto::Window,
    root: xproto::Window,

    /// The child window that the grab confines the cursor to.
    confine: xproto::Window,

    /// The client pointer.
    device: xinput::DeviceId,

    /// The virtual pointer in logical pixels of the window.
    pub position: Point<Pixels>,

    /// The sequence number of the request that starts raw motion, until a
    /// raw event or another event that the server sends after it comes.
    raw_motion_start: Option<u16>,

    /// The buttons that are down, as an XInput 2 button mask.
    buttons: u32,

    /// The width and the height of the screen in device pixels.
    screen: [f32; 2],

    /// The last values of the X and Y axes of each absolute device.
    absolute: BTreeMap<xinput::DeviceId, [Option<f32>; 2]>,
}

impl PointerLock {
    /// Makes the lock of the cursor at `at`, in logical pixels of `window`.
    /// The lock holds the cursor after its grab.
    pub(super) fn new(
        xcb: &XCBConnection,
        root: xproto::Window,
        window: xproto::Window,
        at: Point<Pixels>,
        scale_factor: f32,
    ) -> anyhow::Result<Self> {
        // The core request picks the client pointer, so the server then reports it.
        let pointer = xcb.query_pointer(window)?;
        let client_pointer = xcb.xinput_xi_get_client_pointer(NONE)?;
        // The setup of the connection keeps the size of the screen from before a change of RandR.
        let geometry = xcb.get_geometry(root)?;
        let buttons = core_button_mask(pointer.reply()?.mask);
        let device = client_pointer.reply()?.deviceid;
        let geometry = geometry.reply()?;
        let screen = [f32::from(geometry.width), f32::from(geometry.height)];

        let confine = xcb.generate_id()?;
        let device_pixels = |value: Pixels| (f32::from(value) * scale_factor).floor() as i16;
        check_reply(
            || "X11 CreateWindow of the pointer lock failed.",
            xcb.create_window(
                COPY_DEPTH_FROM_PARENT,
                confine,
                window,
                device_pixels(at.x),
                device_pixels(at.y),
                1,
                1,
                0,
                xproto::WindowClass::INPUT_ONLY,
                COPY_FROM_PARENT,
                &xproto::CreateWindowAux::new(),
            ),
        )?;
        Ok(Self {
            window,
            root,
            confine,
            device,
            position: at,
            raw_motion_start: None,
            buttons,
            screen,
            absolute: BTreeMap::new(),
        })
    }

    /// Starts raw motion and holds the cursor. Ends the grab of a drag that an
    /// earlier lock left.
    pub(super) fn grab(&mut self, xcb: &XCBConnection) -> anyhow::Result<()> {
        let raw_motion = xcb.xinput_xi_select_events(
            self.root,
            &[xinput::EventMask {
                deviceid: XINPUT_ALL_DEVICE_GROUPS,
                mask: vec![xinput::XIEventMask::RAW_MOTION],
            }],
        )?;
        // Events hold the low 16 bits of the sequence number.
        self.raw_motion_start = Some(raw_motion.sequence_number() as u16);
        raw_motion
            .check()
            .context("X11 XiSelectEvents of raw motion failed.")?;
        check_reply(
            || "X11 MapWindow of the pointer lock failed.",
            xcb.map_window(self.confine),
        )?;
        // The press of a drag gives an implicit grab of XInput 2. The server
        // refuses a core grab while a grab of another kind is active.
        check_reply(
            || "X11 XiUngrabDevice failed.",
            xcb.xinput_xi_ungrab_device(CURRENT_TIME, self.device),
        )?;
        // The grab reports events to the windows of this client as usual, so
        // the window keeps its XInput 2 events and the root window its raw
        // events. The server skips the second copy of a raw event on the root
        // window only when the grab window is the root window.
        let status = xcb
            .grab_pointer(
                true,
                self.root,
                xproto::EventMask::BUTTON_PRESS
                    | xproto::EventMask::BUTTON_RELEASE
                    | xproto::EventMask::POINTER_MOTION,
                xproto::GrabMode::ASYNC,
                xproto::GrabMode::ASYNC,
                self.confine,
                NONE,
                CURRENT_TIME,
            )?
            .reply()
            .context("X11 GrabPointer failed.")?
            .status;
        ensure!(
            status == xproto::GrabStatus::SUCCESS,
            "X11 GrabPointer failed: {status:?}"
        );
        Ok(())
    }

    /// Releases the cursor. It stays at the point of the lock. Returns the
    /// device that the window grabs while buttons of a drag stay down.
    #[must_use = "the grab of the drag stays until the client ends it"]
    pub(super) fn release(&self, xcb: &XCBConnection) -> Option<xinput::DeviceId> {
        check_reply(
            || "X11 XiSelectEvents to stop raw motion failed.",
            xcb.xinput_xi_select_events(
                self.root,
                &[xinput::EventMask {
                    deviceid: XINPUT_ALL_DEVICE_GROUPS,
                    mask: vec![0u32.into()],
                }],
            ),
        )
        .log_err();
        check_reply(
            || "X11 UngrabPointer failed.",
            xcb.ungrab_pointer(CURRENT_TIME),
        )
        .log_err();
        let drag = self.buttons != 0 && self.grab_drag(xcb).log_err().is_some();
        check_reply(
            || "X11 DestroyWindow of the pointer lock failed.",
            xcb.destroy_window(self.confine),
        )
        .log_err();
        drag.then_some(self.device)
    }

    /// Gives the window the rest of the drag, as the implicit grab of the
    /// press did before the lock.
    fn grab_drag(&self, xcb: &XCBConnection) -> anyhow::Result<()> {
        let mask = xinput::XIEventMask::MOTION
            | xinput::XIEventMask::BUTTON_PRESS
            | xinput::XIEventMask::BUTTON_RELEASE;
        let status = xcb
            .xinput_xi_grab_device(
                self.window,
                CURRENT_TIME,
                NONE,
                self.device,
                xproto::GrabMode::ASYNC,
                xproto::GrabMode::ASYNC,
                xinput::GrabOwner::NO_OWNER,
                &[mask.into()],
            )?
            .reply()
            .context("X11 XiGrabDevice of the drag failed.")?
            .status;
        ensure!(
            status == xproto::GrabStatus::SUCCESS,
            "X11 XiGrabDevice of the drag failed: {status:?}"
        );
        Ok(())
    }

    /// Notes the buttons of a pointer event of the window.
    pub(super) fn note(&mut self, event: &xinput::ButtonPressEvent, action: PointerAction) {
        self.buttons = buttons_after(event, action);
    }

    /// Moves the virtual pointer to the `position` of a motion event of the
    /// window with the sequence number `sequence`, when the server sent the
    /// event before raw motion started. Returns whether the virtual pointer
    /// moved.
    pub(super) fn move_to(&mut self, sequence: u16, position: Point<Pixels>) -> bool {
        let Some(start) = self.raw_motion_start else {
            return false;
        };
        // The numbers wrap. The events of the queue are much closer than half
        // of the range.
        if sequence.wrapping_sub(start).cast_signed() >= 0 {
            self.raw_motion_start = None;
            return false;
        }
        let moved = position != self.position;
        self.position = position;
        moved
    }

    /// Moves the virtual pointer by the motion of a raw event. The event of
    /// an absolute device has `absolute_axes`, the ranges of its X and Y
    /// axes. Returns whether the virtual pointer moved.
    pub(super) fn move_by(
        &mut self,
        event: &xinput::RawMotionEvent,
        absolute_axes: Option<[AxisRange; 2]>,
        scale_factor: f32,
    ) -> bool {
        // Each later event comes after the start of raw motion.
        self.raw_motion_start = None;
        let value = |axis: usize| {
            get_valuator_axis_index(&event.valuator_mask, axis as u16)
                .and_then(|index| event.axisvalues.get(index))
                .map(|value| fp3232_to_f32(*value))
        };
        let values = [value(0), value(1)];
        let delta = match absolute_axes {
            None => values.map(|value| value.unwrap_or(0.0)),
            Some(axes) => {
                let last = self.absolute.entry(event.sourceid).or_default();
                std::array::from_fn(|axis| {
                    let Some(value) = values[axis] else {
                        return 0.0;
                    };
                    let AxisRange { min, max } = axes[axis];
                    let delta = last[axis]
                        .filter(|_| max > min)
                        .map_or(0.0, |last| (value - last) / (max - min) * self.screen[axis]);
                    last[axis] = Some(value);
                    delta
                })
            }
        };
        if delta == [0.0, 0.0] {
            return false;
        }
        self.position += point(px(delta[0] / scale_factor), px(delta[1] / scale_factor));
        true
    }

    pub(super) fn pressed_button(&self) -> Option<MouseButton> {
        pressed_button_from_mask(self.buttons)
    }
}

/// The XInput 2 button mask of the buttons that are down after a pointer
/// event.
pub(super) fn buttons_after(event: &xinput::ButtonPressEvent, action: PointerAction) -> u32 {
    // The mask of an event holds the buttons from before the event.
    let before = event.button_mask.first().copied().unwrap_or(0);
    let button = 1u32.checked_shl(event.detail).unwrap_or(0);
    match action {
        PointerAction::Press => before | button,
        PointerAction::Release => before & !button,
        PointerAction::Motion => before,
    }
}

/// The ranges of the X and Y axes of each slave pointer with absolute X and
/// Y axes.
pub(super) fn absolute_axes(
    devices: &[xinput::XIDeviceInfo],
) -> BTreeMap<xinput::DeviceId, [AxisRange; 2]> {
    devices
        .iter()
        .filter(|info| info.type_ == xinput::DeviceType::SLAVE_POINTER)
        .filter_map(|info| {
            let axis = |number| {
                info.classes
                    .iter()
                    .filter_map(|class| class.data.as_valuator())
                    .find(|valuator| {
                        valuator.number == number && valuator.mode == xinput::ValuatorMode::ABSOLUTE
                    })
                    .map(AxisRange::of)
            };
            Some((info.deviceid, [axis(0)?, axis(1)?]))
        })
        .collect()
}

/// The XInput 2 button mask of the buttons in a core key and button mask.
fn core_button_mask(mask: xproto::KeyButMask) -> u32 {
    // Core masks start with `Button1` at bit 8. XInput 2 masks start with
    // button 1 at bit 1.
    (u32::from(u16::from(mask)) >> 7) & 0b11_1110
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use x11rb::{
        protocol::{Event, xtest::ConnectionExt as _},
        wrapper::ConnectionExt as _,
    };

    use super::*;
    use crate::linux::x11::{event_position, xvfb::Xvfb};

    /// Reads the events of `xcb` until `done` gives true for them, or until a
    /// second passes.
    fn events_until(xcb: &XCBConnection, done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut events = Vec::new();
        while !done(&events) && Instant::now() < deadline {
            match xcb.poll_for_event().unwrap() {
                Some(event) => events.push(event),
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        events
    }

    fn count_motion(events: &[Event]) -> usize {
        let motion =
            |event: &&Event| matches!(event, Event::XinputMotion(_) | Event::XinputRawMotion(_));
        events.iter().filter(motion).count()
    }

    #[test]
    fn the_motion_of_a_drag_from_its_press_on_moves_the_virtual_pointer() {
        let xvfb = Xvfb::start();
        let (xcb, screen) = xvfb.connect();
        xcb.xinput_xi_query_version(2, 4).unwrap().reply().unwrap();
        let root = xcb.setup().roots[screen].root;
        let window = xcb.generate_id().unwrap();
        xcb.create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            400,
            200,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            COPY_FROM_PARENT,
            &xproto::CreateWindowAux::new(),
        )
        .unwrap();
        xcb.xinput_xi_select_events(
            window,
            &[xinput::EventMask {
                deviceid: XINPUT_ALL_DEVICE_GROUPS,
                mask: vec![
                    xinput::XIEventMask::MOTION
                        | xinput::XIEventMask::BUTTON_PRESS
                        | xinput::XIEventMask::BUTTON_RELEASE,
                ],
            }],
        )
        .unwrap();
        xcb.map_window(window).unwrap();
        xcb.set_input_focus(xproto::InputFocus::PARENT, window, CURRENT_TIME)
            .unwrap();
        xcb.sync().unwrap();

        // Another client sends the input, as a device does.
        let (input, _) = xvfb.connect();
        let fake = |kind, detail, x, y| {
            input
                .xtest_fake_input(kind, detail, CURRENT_TIME, root, x, y, 0)
                .unwrap();
            input.sync().unwrap();
        };
        let move_by = |x| fake(xproto::MOTION_NOTIFY_EVENT, 1, x, 0);
        fake(xproto::MOTION_NOTIFY_EVENT, 0, 50, 40);
        let at = point(px(50.), px(40.));
        events_until(&xcb, |events| count_motion(events) == 1);
        fake(xproto::BUTTON_PRESS_EVENT, 1, 0, 0);
        // The window gets the press and the motion right after it before it locks.
        for _ in 0..3 {
            move_by(20);
        }
        let mut events = events_until(&xcb, |events| count_motion(events) == 3);

        let mut lock = PointerLock::new(&xcb, root, window, at, 1.).unwrap();
        lock.grab(&xcb).unwrap();
        for _ in 0..3 {
            move_by(20);
        }
        events.extend(events_until(&xcb, |events| {
            events
                .iter()
                .filter(|event| matches!(event, Event::XinputRawMotion(_)))
                .count()
                == 3
        }));
        for event in &events {
            match event {
                Event::XinputMotion(event) => {
                    let position = event_position(event.event_x, event.event_y, 1.);
                    lock.move_to(event.sequence, position);
                }
                Event::XinputRawMotion(event) => {
                    lock.move_by(event, None, 1.);
                }
                _ => {}
            }
        }

        assert_eq!(lock.position, point(px(170.), px(40.)));
        let pointer = xcb.query_pointer(window).unwrap().reply().unwrap();
        assert_eq!((pointer.win_x, pointer.win_y), (50, 40), "the cursor stays");
    }

    #[test]
    fn core_buttons_become_xinput_buttons() {
        let mask = xproto::KeyButMask::BUTTON1 | xproto::KeyButMask::BUTTON3;
        assert_eq!(core_button_mask(mask | xproto::KeyButMask::SHIFT), 0b1010);
        assert_eq!(
            pressed_button_from_mask(core_button_mask(mask)),
            Some(MouseButton::Left)
        );
        assert_eq!(core_button_mask(xproto::KeyButMask::BUTTON5), 1 << 5);
    }

    fn raw_motion(source: xinput::DeviceId, mask: u32, values: &[f32]) -> xinput::RawMotionEvent {
        let fp = |value: f32| xinput::Fp3232 {
            integral: value.floor() as i32,
            frac: ((value - value.floor()) * 2f32.powi(32)) as u32,
        };
        xinput::RawMotionEvent {
            sourceid: source,
            valuator_mask: vec![mask],
            axisvalues: values.iter().copied().map(fp).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn pointers_with_absolute_x_and_y_axes_have_axis_ranges() {
        let fp = |integral| xinput::Fp3232 { integral, frac: 0 };
        let valuator = |number, max, mode| xinput::DeviceClass {
            len: 0,
            sourceid: 0,
            data: xinput::DeviceClassData::Valuator(xinput::DeviceClassDataValuator {
                number,
                label: 0,
                min: fp(0),
                max: fp(max),
                value: fp(0),
                resolution: 0,
                mode,
            }),
        };
        let device = |deviceid, type_, classes| xinput::XIDeviceInfo {
            deviceid,
            type_,
            attachment: 0,
            enabled: true,
            name: Vec::new(),
            classes,
        };
        let absolute = xinput::ValuatorMode::ABSOLUTE;
        let relative = xinput::ValuatorMode::RELATIVE;
        let slave = xinput::DeviceType::SLAVE_POINTER;
        let devices = [
            device(
                9,
                slave,
                vec![valuator(0, -1, relative), valuator(1, -1, relative)],
            ),
            device(
                10,
                slave,
                vec![valuator(0, 32767, absolute), valuator(1, 16383, absolute)],
            ),
            device(11, slave, vec![valuator(0, 32767, absolute)]),
            device(
                2,
                xinput::DeviceType::MASTER_POINTER,
                vec![valuator(0, 32767, absolute), valuator(1, 16383, absolute)],
            ),
        ];
        let range = |max| AxisRange { min: 0., max };
        assert_eq!(
            absolute_axes(&devices),
            BTreeMap::from([(10, [range(32767.), range(16383.)])])
        );
    }

    #[test]
    fn a_mouse_and_a_pen_move_the_virtual_pointer() {
        const MOUSE: xinput::DeviceId = 9;
        const PEN: xinput::DeviceId = 12;
        let mut lock = PointerLock {
            window: 1,
            root: 2,
            confine: 3,
            device: 4,
            position: point(px(100.), px(50.)),
            raw_motion_start: None,
            buttons: 0,
            screen: [1920., 1080.],
            absolute: BTreeMap::new(),
        };
        let axes = Some([
            AxisRange {
                min: 0.,
                max: 40000.,
            },
            AxisRange {
                min: 0.,
                max: 20000.,
            },
        ]);

        assert!(lock.move_by(&raw_motion(MOUSE, 0b11, &[6., -4.]), None, 2.));
        assert_eq!(lock.position, point(px(103.), px(48.)));
        assert!(!lock.move_by(&raw_motion(MOUSE, 0b100, &[1.]), None, 2.));

        // The first event of a pen gives its values, and no motion.
        assert!(!lock.move_by(&raw_motion(PEN, 0b111, &[10000., 10000., 512.]), axes, 2.));
        assert_eq!(lock.position, point(px(103.), px(48.)));
        // A pen that moves a tenth of its X range moves a tenth of the screen.
        assert!(lock.move_by(&raw_motion(PEN, 0b1, &[14000.]), axes, 2.));
        assert_eq!(lock.position, point(px(199.), px(48.)));
        assert!(lock.move_by(&raw_motion(PEN, 0b10, &[9000.]), axes, 2.));
        assert_eq!(lock.position, point(px(199.), px(21.)));
    }
}
