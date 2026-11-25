// The raw mouse input handling ports parts of `WIN_HandleRawMouseInput` of
// `src/video/windows/SDL_windowsevents.c` and of the system scale of
// `src/video/windows/SDL_windowsmouse.c` of SDL 3: https://github.com/libsdl-org/SDL
//
// Simple DirectMedia Layer
// Copyright (C) 1997-2026 Sam Lantinga <slouken@libsdl.org>
//
// This software is provided 'as-is', without any express or implied
// warranty.  In no event will the authors be held liable for any damages
// arising from the use of this software.
//
// Permission is granted to anyone to use this software for any purpose,
// including commercial applications, and to alter it and redistribute it
// freely, subject to the following restrictions:
//
// 1. The origin of this software must not be misrepresented; you must not
//    claim that you wrote the original software. If you use this software
//    in a product, an acknowledgment in the product documentation would be
//    appreciated but is not required.
// 2. Altered source versions must be plainly marked as such, and must not be
//    misrepresented as being the original software.
// 3. This notice may not be removed or altered from any source distribution.

use std::{ffi::c_void, mem::size_of};

use gpui::{Pixels, Point, point, px};
use gpui_util::ResultExt;
use windows::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, POINT, RECT},
    Graphics::Gdi::ClientToScreen,
    UI::{
        Input::*,
        WindowsAndMessaging::{
            ClipCursor, GetClipCursor, GetSystemMetrics, SM_CXSCREEN, SM_CXVIRTUALSCREEN,
            SM_CYSCREEN, SM_CYVIRTUALSCREEN, SPI_GETMOUSESPEED,
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
        },
    },
};

/// The HID usage of a mouse: <https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/hid-usages#usage-page>.
const HID_USAGE_PAGE_GENERIC: u16 = 1;
const HID_USAGE_GENERIC_MOUSE: u16 = 2;

/// SDL reads an absolute packet with this flag as a position in screen pixels.
const MOUSE_RAW_COORDINATES: u16 = 0x40;

/// Holds the system cursor at one point of the screen, and moves a virtual pointer by the
/// relative motion of the mouse.
#[derive(Clone, Copy)]
pub(crate) struct PointerLock {
    /// The rect of one screen pixel that the cursor is clipped to.
    clip: RECT,

    /// The virtual pointer, in logical pixels of the client area.
    pub(crate) position: Point<Pixels>,

    /// The screen pixels that one count of relative motion moves the pointer.
    pixels_per_count: f32,

    /// The last absolute device and its last position, in screen pixels.
    last_absolute: Option<(HANDLE, POINT)>,
}

impl PointerLock {
    /// Clips the cursor to `at` in the client area of the window and listens to the raw mouse
    /// input.
    pub(crate) fn hold(hwnd: HWND, at: Point<Pixels>, scale_factor: f32) -> Option<Self> {
        let mut origin = POINT {
            x: (at.x.as_f32() * scale_factor).round() as i32,
            y: (at.y.as_f32() * scale_factor).round() as i32,
        };
        if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
            return None;
        }
        let clip = RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + 1,
            bottom: origin.y + 1,
        };
        register_raw_mouse(RAWINPUTDEVICE_FLAGS(0), hwnd).log_err()?;
        if unsafe { ClipCursor(Some(&clip)) }.log_err().is_none() {
            register_raw_mouse(RIDEV_REMOVE, HWND::default()).log_err();
            return None;
        }
        Some(Self {
            clip,
            position: at,
            pixels_per_count: pixels_per_count(),
            last_absolute: None,
        })
    }

    /// Clips the cursor again when other code freed it, such as another process or a change
    /// of the desktop.
    pub(crate) fn restore_clip(&self) {
        let mut clip = RECT::default();
        if unsafe { GetClipCursor(&mut clip) }.is_ok() && clip != self.clip {
            unsafe { ClipCursor(Some(&self.clip)) }.log_err();
        }
    }

    /// Stops the raw mouse input, and frees the cursor unless other code clipped it since.
    pub(crate) fn release(&self) {
        let mut clip = RECT::default();
        if unsafe { GetClipCursor(&mut clip) }.is_ok() && clip == self.clip {
            unsafe { ClipCursor(None) }.log_err();
        }
        register_raw_mouse(RIDEV_REMOVE, HWND::default()).log_err();
    }

    /// Moves the virtual pointer by the motion of the packet of a `WM_INPUT` message.
    /// Returns whether the pointer moved.
    pub(crate) fn move_by_raw_input(&mut self, lparam: LPARAM, scale_factor: f32) -> bool {
        let mut input = RAWINPUT::default();
        let mut size = size_of::<RAWINPUT>() as u32;
        let read = unsafe {
            GetRawInputData(
                HRAWINPUT(lparam.0 as *mut c_void),
                RID_INPUT,
                Some(&mut input as *mut RAWINPUT as *mut c_void),
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if read == u32::MAX || input.header.dwType != RIM_TYPEMOUSE.0 {
            return false;
        }
        let mouse = unsafe { input.data.mouse };
        let delta = if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 == 0 {
            point(mouse.lLastX as f32, mouse.lLastY as f32) * self.pixels_per_count / scale_factor
        } else {
            // Pens, touch, RDP, virtual machines and tablets send positions. The motion is the
            // change of the position.
            // The first packet of a device gives no motion.
            let device = input.header.hDevice;
            let at = absolute_position(&mouse);
            let last = match self.last_absolute.replace((device, at)) {
                Some((last_device, last)) if last_device == device => last,
                _ => at,
            };
            point((at.x - last.x) as f32, (at.y - last.y) as f32) / scale_factor
        };
        if delta.x == 0.0 && delta.y == 0.0 {
            return false;
        }
        self.position += point(px(delta.x), px(delta.y));
        true
    }
}

fn register_raw_mouse(flags: RAWINPUTDEVICE_FLAGS, hwnd: HWND) -> windows::core::Result<()> {
    let device = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_MOUSE,
        dwFlags: flags,
        hwndTarget: hwnd,
    };
    unsafe { RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32) }
}

/// The position of an absolute packet, in screen pixels.
fn absolute_position(mouse: &RAWMOUSE) -> POINT {
    if mouse.usFlags.0 & MOUSE_RAW_COORDINATES != 0 {
        return POINT {
            x: mouse.lLastX,
            y: mouse.lLastY,
        };
    }
    let (width, height) = if mouse.usFlags.0 & MOUSE_VIRTUAL_DESKTOP.0 != 0 {
        (SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN)
    } else {
        (SM_CXSCREEN, SM_CYSCREEN)
    };
    let (width, height) = unsafe { (GetSystemMetrics(width), GetSystemMetrics(height)) };
    POINT {
        x: (mouse.lLastX as f32 / 65535.0 * width as f32) as i32,
        y: (mouse.lLastY as f32 / 65535.0 * height as f32) as i32,
    }
}

/// The screen pixels that one count of relative motion moves the cursor, at the pointer speed
/// of the system and with no acceleration.
fn pixels_per_count() -> f32 {
    let mut speed: i32 = 10;
    unsafe {
        SystemParametersInfoW(
            SPI_GETMOUSESPEED,
            0,
            Some(&mut speed as *mut i32 as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
        )
    }
    .log_err();
    speed_factor(speed)
}

/// The factor of the pointer speed of the system, from 1 to 20, as SDL computes it.
fn speed_factor(speed: i32) -> f32 {
    let speed = speed.clamp(1, 20);
    speed.max((speed - 2) * 4).max((speed - 6) * 8) as f32 / 32.0
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{DispatchEventResult, MouseButton, PlatformInput, point, px};
    use windows::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE,
                MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
                MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
            },
            WindowsAndMessaging::{
                ClipCursor, GetClipCursor, GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN,
                SM_CYVIRTUALSCREEN, SPI_GETMOUSE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
                SetCursorPos, SystemParametersInfoW,
            },
        },
    };

    use super::{pixels_per_count, speed_factor};
    use crate::test_window::{self, pump_until};

    #[test]
    fn the_speed_factor_follows_the_table_of_windows() {
        let factors: Vec<f32> = (1..=20).map(speed_factor).collect();
        assert_eq!(
            factors,
            [
                0.03125, 0.0625, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0, 1.25, 1.5, 1.75,
                2.0, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5
            ]
        );
    }

    type Events = Rc<RefCell<Vec<PlatformInput>>>;

    /// Frees the cursor when a test fails while it holds the cursor.
    struct Unclip;

    impl Drop for Unclip {
        fn drop(&mut self) {
            unsafe { ClipCursor(None) }.ok();
        }
    }

    /// The extra information of the mouse input of a pen:
    /// <https://learn.microsoft.com/en-us/windows/win32/tablet/system-events-and-mouse-messages>.
    const MI_WP_SIGNATURE: usize = 0xFF51_5700;

    fn send_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS, extra_info: usize) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    dwFlags: flags,
                    dwExtraInfo: extra_info,
                    ..Default::default()
                },
            },
        };
        let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
        assert_eq!(sent, 1, "SendInput failed");
    }

    fn send_pen(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) {
        send_input(dx, dy, flags, MI_WP_SIGNATURE);
    }

    /// Sends the mouse input and waits for the window to report a mouse event.
    fn send_and_take(events: &Events, dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> PlatformInput {
        take_after(events, || send_input(dx, dy, flags, 0))
    }

    /// Sends the pen input and waits for the window to report a mouse event.
    fn send_pen_and_take(
        events: &Events,
        dx: i32,
        dy: i32,
        flags: MOUSE_EVENT_FLAGS,
    ) -> PlatformInput {
        take_after(events, || send_pen(dx, dy, flags))
    }

    /// Runs `send` and waits for the window to report a mouse event.
    fn take_after(events: &Events, send: impl FnOnce()) -> PlatformInput {
        events.borrow_mut().clear();
        send();
        let is_mouse = |event: &PlatformInput| {
            matches!(
                event,
                PlatformInput::MouseMove(_)
                    | PlatformInput::MouseDown(_)
                    | PlatformInput::MouseUp(_)
            )
        };
        assert!(
            pump_until(|| events.borrow().iter().any(is_mouse)),
            "the window reported no event of the mouse input"
        );
        let index = events.borrow().iter().position(is_mouse).unwrap();
        events.borrow_mut().remove(index)
    }

    fn cursor() -> POINT {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }.unwrap();
        point
    }

    fn enhanced_pointer_precision() -> bool {
        let mut params = [0i32; 3];
        unsafe {
            SystemParametersInfoW(
                SPI_GETMOUSE,
                0,
                Some(params.as_mut_ptr().cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
            )
        }
        .unwrap();
        params[2] != 0
    }

    #[test]
    fn the_pointer_lock_holds_the_cursor_while_the_mouse_moves_a_virtual_pointer() {
        let (_platform, window) = test_window::open(test_window::params());
        let _unclip = Unclip;
        let events = Events::default();
        window.on_input(Box::new({
            let events = events.clone();
            move |input| {
                events.borrow_mut().push(input);
                DispatchEventResult::default()
            }
        }));
        window.activate();
        assert!(
            pump_until(|| window.is_active()),
            "the window is not active"
        );

        let scale_factor = window.scale_factor();
        let at = point(px(200.0), px(150.0));
        let mut held = POINT {
            x: (200.0 * scale_factor).round() as i32,
            y: (150.0 * scale_factor).round() as i32,
        };
        assert!(unsafe { ClientToScreen(window.get_raw_handle(), &mut held) }.as_bool());
        unsafe { SetCursorPos(held.x, held.y) }.unwrap();

        assert!(!window.set_pointer_lock(None));
        assert!(window.set_pointer_lock(Some(at)));
        let mut clip = RECT::default();
        unsafe { GetClipCursor(&mut clip) }.unwrap();
        assert_eq!(
            (clip.left, clip.top, clip.right, clip.bottom),
            (held.x, held.y, held.x + 1, held.y + 1)
        );

        let per_count = pixels_per_count();
        // The logical motion of the counts, in the order of the arithmetic of the lock.
        let motion = |dx: f32, dy: f32| {
            point(
                px(dx * per_count / scale_factor),
                px(dy * per_count / scale_factor),
            )
        };
        let mut expected = at;
        for _ in 0..10 {
            let PlatformInput::MouseMove(event) = send_and_take(&events, 7, -3, MOUSEEVENTF_MOVE)
            else {
                panic!("the window reported no mouse move");
            };
            expected += motion(7.0, -3.0);
            assert_eq!(event.position, expected);
            assert_eq!(event.pressed_button, None);
            assert_eq!(window.mouse_position(), expected);
            assert_eq!(cursor(), held);
        }

        // The lock clips the cursor again when other code frees it.
        unsafe { ClipCursor(None) }.unwrap();
        unsafe { SetCursorPos(held.x + 10, held.y) }.unwrap();
        assert!(
            pump_until(|| cursor() == held),
            "the lock does not clip the cursor again"
        );

        let PlatformInput::MouseDown(event) = send_and_take(&events, 0, 0, MOUSEEVENTF_LEFTDOWN)
        else {
            panic!("the window reported no mouse down");
        };
        assert_eq!(event.position, expected);
        let PlatformInput::MouseMove(event) = send_and_take(&events, 5, 5, MOUSEEVENTF_MOVE) else {
            panic!("the window reported no mouse move");
        };
        expected += motion(5.0, 5.0);
        assert_eq!(event.position, expected);
        assert_eq!(event.pressed_button, Some(MouseButton::Left));
        let PlatformInput::MouseUp(event) = send_and_take(&events, 0, 0, MOUSEEVENTF_LEFTUP) else {
            panic!("the window reported no mouse up");
        };
        assert_eq!(event.position, expected);
        assert_eq!(cursor(), held);

        // A pen moves the virtual pointer by the change of its position.
        let pen = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        let (width, height) = unsafe {
            (
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        let screen_pixel = |value: i32, size: i32| (value as f32 / 65535.0 * size as f32) as i32;
        send_pen(20_000, 20_000, pen);
        let PlatformInput::MouseMove(event) = send_pen_and_take(&events, 21_000, 19_000, pen)
        else {
            panic!("the window reported no mouse move of the pen");
        };
        expected += point(
            px((screen_pixel(21_000, width) - screen_pixel(20_000, width)) as f32 / scale_factor),
            px((screen_pixel(19_000, height) - screen_pixel(20_000, height)) as f32 / scale_factor),
        );
        assert_eq!(event.position, expected);
        assert_eq!(cursor(), held);

        assert!(!window.set_pointer_lock(None));
        assert_eq!(cursor(), held);
        unsafe { GetClipCursor(&mut clip) }.unwrap();
        assert_ne!(
            (clip.left, clip.top, clip.right, clip.bottom),
            (held.x, held.y, held.x + 1, held.y + 1)
        );
        assert_eq!(window.mouse_position(), at);

        // The virtual pointer moves as far as the cursor does with no lock.
        if enhanced_pointer_precision() {
            eprintln!("skipped the comparison with the cursor: enhanced pointer precision is on");
        } else {
            send_and_take(&events, 40, 0, MOUSEEVENTF_MOVE);
            let moved = cursor().x - held.x;
            eprintln!(
                "40 counts moved the cursor {moved} pixels at scale {scale_factor} and speed factor {per_count}"
            );
            assert_eq!(moved as f32, (40.0 * per_count).round());
        }

        test_window::close(window);
    }
}
