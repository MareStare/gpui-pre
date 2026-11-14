//! Pen input of a window.
//!
//! Pen input comes as `WM_POINTER*` messages of Windows Ink first. The
//! window gives each pen sample to its pen callback, then passes the
//! message on. The default window procedure makes mouse messages from it.
//!
//! Windows puts the samples of a fast pen into one message and keeps them in
//! the history of the pointer. Mouse messages have whole pixels, so the
//! samples take their positions from the HIMETRIC location of the pointer.
//!
//! Tablet drivers can turn off Windows Ink. Then the pen moves the mouse,
//! and the samples come only through WinTab. The app picks the API with
//! [`PenApi`]. The messages of the other API give no pen events. With
//! [`PenApi::Auto`], the windows read WinTab packets and proximity until
//! Windows Ink gives the first touch of a pen. Such a driver can still send
//! a Windows Ink message of a hovering pen.

use std::{cell::Cell, rc::Rc};

use gpui::{PenApi, PenEvent, Point, point, px};
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        UI::{
            HiDpi::GetDpiForWindow,
            Input::Pointer::{
                GetPointerDeviceRects, GetPointerPenInfo, GetPointerPenInfoHistory, GetPointerType,
                POINTER_FLAG_INCONTACT, POINTER_PEN_INFO,
            },
            WindowsAndMessaging::{
                PEN_MASK_PRESSURE, POINTER_INPUT_TYPE, PT_PEN, SetPropW, WA_INACTIVE, WM_ACTIVATE,
                WM_NCDESTROY, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERLEAVE,
                WM_POINTERUP, WM_POINTERUPDATE,
            },
        },
    },
    core::w,
};

use crate::{
    HiLoWord,
    wintab::{self, Wintab},
};

/// The pen API of the app. All windows share it.
#[derive(Default)]
pub(crate) struct PenApiState {
    api: Cell<PenApi>,

    /// Windows Ink gave a touch of a pen. [`PenApi::Auto`] ignores WinTab
    /// messages after that.
    ink_touched: Cell<bool>,
}

impl PenApiState {
    pub(crate) fn set(&self, api: PenApi) {
        self.api.set(api);
    }

    /// The API can read WinTab.
    fn wintab_wanted(&self) -> bool {
        match self.api.get() {
            PenApi::Auto | PenApi::Wintab => true,
            PenApi::WindowsInk => false,
        }
    }

    /// The messages of Windows Ink give pen events.
    fn ink_on(&self) -> bool {
        match self.api.get() {
            PenApi::Auto | PenApi::WindowsInk => true,
            PenApi::Wintab => false,
        }
    }

    /// The messages of WinTab give pen events.
    fn wintab_on(&self) -> bool {
        match self.api.get() {
            PenApi::Auto => !self.ink_touched.get(),
            PenApi::Wintab => true,
            PenApi::WindowsInk => false,
        }
    }
}

/// The pen of one window.
pub(crate) struct WindowPen {
    api: Rc<PenApiState>,
    callback: Cell<Option<Box<dyn FnMut(PenEvent)>>>,

    /// The app reads the pen of the window. A window that does not reads no
    /// pen messages and opens no WinTab context.
    listening: Cell<bool>,
    wintab: Wintab,
}

/// Values of the `MicrosoftTabletPenServiceProperty` window property.
/// They turn off "press and hold for right click", tap feedback, barrel
/// button feedback and flicks. These get in the way of painting.
const TABLET_FLAGS: usize = 0x0000_0001 | 0x0000_0008 | 0x0000_0010 | 0x0001_0000;

impl WindowPen {
    pub(crate) fn new(api: Rc<PenApiState>) -> Self {
        Self {
            api,
            callback: Cell::new(None),
            listening: Cell::new(false),
            wintab: Wintab::new(wintab::api),
        }
    }

    /// Gives each pen event of the window to `callback`. It turns off the
    /// pen feedback of the window and opens a WinTab context when the API
    /// reads WinTab.
    pub(crate) fn listen(&self, hwnd: HWND, callback: Box<dyn FnMut(PenEvent)>) {
        // A window that keeps the feedback still gives the pen events.
        unsafe {
            SetPropW(
                hwnd,
                w!("MicrosoftTabletPenServiceProperty"),
                Some(HANDLE(TABLET_FLAGS as *mut _)),
            )
        }
        .ok();
        self.callback.set(Some(callback));
        self.listening.set(true);
        self.wintab.want(hwnd, self.api.wintab_wanted());
    }

    /// Applies a change of the pen API. The pen leaves, so no state of the
    /// API before stays.
    pub(crate) fn use_api(&self, hwnd: HWND) {
        if self.listening.get() {
            self.emit_one(PenEvent::Leave);
            self.wintab.want(hwnd, self.api.wintab_wanted());
        }
    }

    /// Answers [`crate::WM_GPUI_WINTAB_OPENED`].
    pub(crate) fn handle_wintab_opened(&self) -> Option<isize> {
        self.wintab.on_opened();
        Some(0)
    }

    /// Reads the pen input from a message of the window. The message goes
    /// on to the window procedure after it.
    pub(crate) fn handle_msg(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) {
        if !self.listening.get() {
            return;
        }
        match msg {
            WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
                if let Some(history) = read_history(wparam)
                    && let Some(newest) = history.first()
                {
                    let map = WindowMap::read(hwnd, Some(newest.pointerInfo.sourceDevice));
                    self.on_ink(&history, &map);
                }
            }
            WM_POINTERLEAVE | WM_POINTERCAPTURECHANGED if self.api.ink_on() => {
                self.emit_one(PenEvent::Leave)
            }
            wintab::WT_PACKET | wintab::WT_PROXIMITY if !self.api.wintab_on() => {}
            wintab::WT_PACKET => self.emit(|emit| self.wintab.read_packets(hwnd, lparam, emit)),
            // The low word is zero when the pen leaves the tablet.
            wintab::WT_PROXIMITY if lparam.loword() == 0 => self.emit_one(PenEvent::Leave),
            wintab::WT_PROXIMITY => self.emit_one(PenEvent::Hover),
            WM_ACTIVATE if u32::from(wparam.loword()) == WA_INACTIVE => {
                self.emit_one(PenEvent::Leave)
            }
            WM_ACTIVATE => self.wintab.bring_to_top(),
            WM_NCDESTROY => self.wintab.want(hwnd, false),
            _ => {}
        }
    }

    /// Gives the pen states of one Windows Ink message.
    fn on_ink(&self, history: &[POINTER_PEN_INFO], map: &WindowMap) {
        if !self.api.ink_on() {
            return;
        }
        if history.iter().any(touches) {
            self.api.ink_touched.set(true);
        }
        self.emit(|emit| ink_events(history, map, emit));
    }

    fn emit_one(&self, event: PenEvent) {
        self.emit(|emit| emit(event));
    }

    fn emit(&self, events: impl FnOnce(&mut dyn FnMut(PenEvent))) {
        let Some(mut callback) = self.callback.take() else {
            return;
        };
        events(&mut *callback);
        self.callback.set(Some(callback));
    }
}

fn touches(info: &POINTER_PEN_INFO) -> bool {
    info.pointerInfo
        .pointerFlags
        .contains(POINTER_FLAG_INCONTACT)
}

/// Maps positions on the screen to logical pixels of a window.
#[derive(Clone, Copy)]
pub(crate) struct WindowMap {
    /// For each axis, the screen pixels per HIMETRIC unit and the screen
    /// pixel of HIMETRIC 0. `None` when the device gives no rectangles.
    himetric: Option<[(f64, f64); 2]>,

    /// The top left corner of the client area on the screen.
    origin: POINT,

    /// Physical pixels per logical pixel.
    scale: f64,
}

impl WindowMap {
    /// A HIMETRIC position is wrong when it maps farther than this from the
    /// pixel of the pointer, in screen pixels. The pixel is used then.
    const TOLERANCE: f64 = 2.0;

    /// `device` is the HIMETRIC rectangle of the pen device and its
    /// rectangle on the screen.
    pub(crate) fn new(device: Option<(RECT, RECT)>, origin: POINT, scale: f64) -> Self {
        let himetric = device.and_then(|(himetric, screen)| {
            let axis = |h0: i32, h1: i32, s0: i32, s1: i32| {
                let k = f64::from(s1 - s0) / f64::from(h1 - h0);
                k.is_finite()
                    .then(|| (k, f64::from(s0) - k * f64::from(h0)))
            };
            Some([
                axis(himetric.left, himetric.right, screen.left, screen.right)?,
                axis(himetric.top, himetric.bottom, screen.top, screen.bottom)?,
            ])
        });
        Self {
            himetric,
            origin,
            scale,
        }
    }

    /// Reads the map of the window, with the HIMETRIC rectangles of the
    /// pen `device`.
    pub(crate) fn read(hwnd: HWND, device: Option<HANDLE>) -> Self {
        let mut himetric = RECT::default();
        let mut screen = RECT::default();
        let mut origin = POINT { x: 0, y: 0 };
        let (rects, dpi) = unsafe {
            let rects = device.is_some_and(|device| {
                GetPointerDeviceRects(device, &raw mut himetric, &raw mut screen).is_ok()
            });
            _ = ClientToScreen(hwnd, &raw mut origin);
            (rects.then_some((himetric, screen)), GetDpiForWindow(hwnd))
        };
        // 96 DPI is a scale of 1.
        let scale = if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 };
        Self::new(rects, origin, scale)
    }

    /// The position in logical pixels of the window.
    pub(crate) fn screen_to_window(&self, (x, y): (f64, f64)) -> Point<gpui::Pixels> {
        let x = (x - f64::from(self.origin.x)) / self.scale;
        let y = (y - f64::from(self.origin.y)) / self.scale;
        point(px(x as f32), px(y as f32))
    }

    /// The position in logical pixels of the window, from the HIMETRIC
    /// location of the pointer and its pixel.
    fn himetric_to_window(&self, himetric: POINT, pixel: POINT) -> Point<gpui::Pixels> {
        let pixel = (f64::from(pixel.x), f64::from(pixel.y));
        let screen = self
            .himetric
            .map(|[(kx, ox), (ky, oy)]| {
                (
                    kx * f64::from(himetric.x) + ox,
                    ky * f64::from(himetric.y) + oy,
                )
            })
            .filter(|&(x, y)| {
                (x - pixel.0).abs() <= Self::TOLERANCE && (y - pixel.1).abs() <= Self::TOLERANCE
            })
            .unwrap_or(pixel);
        self.screen_to_window(screen)
    }
}

/// The pen states of one pointer message, newest first, as
/// `GetPointerPenInfoHistory` gives them. `None` when the pointer is not a
/// pen.
fn read_history(wparam: WPARAM) -> Option<Vec<POINTER_PEN_INFO>> {
    let id = u32::from(wparam.loword());
    let mut kind = POINTER_INPUT_TYPE::default();
    if unsafe { GetPointerType(id, &raw mut kind) }.is_err() || kind != PT_PEN {
        return None;
    }
    let mut info = POINTER_PEN_INFO::default();
    unsafe { GetPointerPenInfo(id, &raw mut info) }.ok()?;
    let mut count = info.pointerInfo.historyCount;
    if count <= 1 {
        return Some(vec![info]);
    }
    let mut history = vec![POINTER_PEN_INFO::default(); count as usize];
    // The buffer has room for `count` entries.
    let read = unsafe { GetPointerPenInfoHistory(id, &raw mut count, Some(history.as_mut_ptr())) };
    if read.is_err() {
        return Some(vec![info]);
    }
    history.truncate(count as usize);
    Some(history)
}

/// Gives the pen states of one message, oldest first.
fn ink_events(history: &[POINTER_PEN_INFO], map: &WindowMap, emit: &mut dyn FnMut(PenEvent)) {
    for info in history.iter().rev() {
        if !touches(info) {
            emit(PenEvent::Hover);
            continue;
        }
        let pointer = &info.pointerInfo;
        let position =
            map.himetric_to_window(pointer.ptHimetricLocationRaw, pointer.ptPixelLocationRaw);
        // Windows gives the pressure in `0..=1024`. A pen with no pressure
        // paints at full pressure.
        let pressure = if info.penMask & PEN_MASK_PRESSURE == 0 {
            1.0
        } else {
            (info.pressure as f32 / 1024.0).clamp(0.0, 1.0)
        };
        emit(PenEvent::Touch { position, pressure });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use windows::Win32::UI::Input::Pointer::{POINTER_FLAGS, POINTER_INFO};

    type Events = Rc<RefCell<Vec<PenEvent>>>;

    /// A window that listens to its pen, with no WinTab driver.
    fn window(api: &Rc<PenApiState>) -> (WindowPen, Events) {
        let events = Events::default();
        let pen = WindowPen {
            api: api.clone(),
            callback: Cell::new(None),
            listening: Cell::new(true),
            wintab: Wintab::new(|| None),
        };
        let sink = events.clone();
        pen.callback
            .set(Some(Box::new(move |event| sink.borrow_mut().push(event))));
        (pen, events)
    }

    fn take(events: &Events) -> Vec<PenEvent> {
        std::mem::take(&mut events.borrow_mut())
    }

    fn touch(x: f32, y: f32, pressure: f32) -> PenEvent {
        PenEvent::Touch {
            position: point(px(x), px(y)),
            pressure,
        }
    }

    const HWND0: HWND = HWND(std::ptr::null_mut());
    const ENTER: LPARAM = LPARAM(1);
    const OUT: LPARAM = LPARAM(0);

    #[test]
    fn the_pen_leaves_on_capture_change_out_of_proximity_and_on_deactivation() {
        let (pen, events) = window(&Rc::default());
        let msg = |msg, wparam, lparam| pen.handle_msg(HWND0, msg, WPARAM(wparam), lparam);
        // Pointer 0 is not a pen.
        msg(WM_POINTERUPDATE, 0, OUT);
        msg(WM_ACTIVATE, 1, OUT);
        assert_eq!(take(&events), []);
        msg(WM_POINTERCAPTURECHANGED, 0, OUT);
        assert_eq!(take(&events), [PenEvent::Leave]);
        msg(wintab::WT_PROXIMITY, 0, ENTER);
        msg(wintab::WT_PROXIMITY, 0, OUT);
        assert_eq!(take(&events), [PenEvent::Hover, PenEvent::Leave]);
        msg(WM_ACTIVATE, WA_INACTIVE as usize, OUT);
        msg(WM_POINTERLEAVE, 0, OUT);
        assert_eq!(take(&events), [PenEvent::Leave, PenEvent::Leave]);
    }

    #[test]
    fn a_window_that_does_not_listen_gives_no_events() {
        let (pen, events) = window(&Rc::default());
        pen.listening.set(false);
        pen.handle_msg(HWND0, wintab::WT_PROXIMITY, WPARAM(0), ENTER);
        pen.use_api(HWND0);
        assert_eq!(take(&events), []);
    }

    /// One state of the pointer history.
    fn state(himetric: (i32, i32), pixel: (i32, i32), pressure: u32) -> POINTER_PEN_INFO {
        POINTER_PEN_INFO {
            pointerInfo: POINTER_INFO {
                pointerFlags: if pressure > 0 {
                    POINTER_FLAG_INCONTACT
                } else {
                    POINTER_FLAGS(0)
                },
                ptHimetricLocationRaw: POINT {
                    x: himetric.0,
                    y: himetric.1,
                },
                ptPixelLocationRaw: POINT {
                    x: pixel.0,
                    y: pixel.1,
                },
                ..POINTER_INFO::default()
            },
            penMask: PEN_MASK_PRESSURE,
            pressure,
            ..POINTER_PEN_INFO::default()
        }
    }

    const fn rect(right: i32, bottom: i32) -> RECT {
        RECT {
            left: 0,
            top: 0,
            right,
            bottom,
        }
    }

    fn collect(history: &[POINTER_PEN_INFO], map: &WindowMap) -> Vec<PenEvent> {
        let mut events = Vec::new();
        ink_events(history, map, &mut |event| events.push(event));
        events
    }

    #[test]
    fn a_fast_pen_gives_every_sample_of_its_history_in_order() {
        // The device spans 20000 HIMETRIC units and the screen 2000 pixels:
        // 0.1 pixel per unit. The client area starts at (100, 50), and the
        // display has a scale of 150%.
        let map = WindowMap::new(
            Some((rect(20000, 10000), rect(2000, 1000))),
            POINT { x: 100, y: 50 },
            1.5,
        );
        // A fast curve: 12 samples in one message, and the lift after them.
        let path = (0..12i32)
            .map(|i| (2003 + i * 157, 1007 + i * i * 31, 100 + 50 * i as u32))
            .collect::<Vec<_>>();
        let mut history = path
            .iter()
            .map(|&(hx, hy, p)| state((hx, hy), (hx / 10, hy / 10), p))
            .collect::<Vec<_>>();
        history.push(state((9000, 9000), (900, 900), 0));
        // Windows gives the history newest first.
        history.reverse();
        let events = collect(&history, &map);
        assert_eq!(events.last(), Some(&PenEvent::Hover));
        let samples = &events[..events.len() - 1];
        assert_eq!(samples.len(), path.len());
        for (sample, &(hx, hy, p)) in samples.iter().zip(&path) {
            let PenEvent::Touch { position, pressure } = *sample else {
                panic!("{sample:?}");
            };
            let x = (f64::from(hx) / 10.0 - 100.0) / 1.5;
            let y = (f64::from(hy) / 10.0 - 50.0) / 1.5;
            assert!(
                (f64::from(position.x.as_f32()) - x).abs() < 1e-3,
                "{sample:?} {x}"
            );
            assert!(
                (f64::from(position.y.as_f32()) - y).abs() < 1e-3,
                "{sample:?} {y}"
            );
            assert_eq!(pressure, p as f32 / 1024.0);
        }
        assert!(samples.iter().any(|sample| matches!(
            sample,
            PenEvent::Touch { position, .. } if position.x.as_f32().fract() != 0.0
        )));
    }

    #[test]
    fn a_himetric_position_far_from_the_pixel_falls_back_to_the_pixel() {
        let origin = POINT { x: 0, y: 0 };
        // The device rectangle maps to another area than the pixels.
        let wrong = WindowMap::new(Some((rect(1000, 1000), rect(4000, 4000))), origin, 1.0);
        let pos = wrong.himetric_to_window(POINT { x: 100, y: 100 }, POINT { x: 30, y: 40 });
        assert_eq!(pos, point(px(30.0), px(40.0)));
        let none = WindowMap::new(None, POINT { x: 10, y: 0 }, 2.0);
        let pos = none.himetric_to_window(POINT { x: 100, y: 100 }, POINT { x: 30, y: 40 });
        assert_eq!(pos, point(px(10.0), px(20.0)));
        let flat = WindowMap::new(Some((rect(0, 0), rect(10, 10))), origin, 1.0);
        assert!(flat.himetric.is_none());
    }

    #[test]
    fn a_hover_of_windows_ink_keeps_the_wintab_pen_and_a_touch_ends_it() {
        let api = Rc::default();
        let (pen, events) = window(&api);
        let map = WindowMap::new(None, POINT { x: 0, y: 0 }, 1.0);
        // A driver with no Windows Ink sends one message of a hovering pen.
        pen.on_ink(&[state((0, 0), (5, 6), 0)], &map);
        pen.handle_msg(HWND0, wintab::WT_PROXIMITY, WPARAM(0), ENTER);
        assert_eq!(take(&events), [PenEvent::Hover, PenEvent::Hover]);
        // Windows Ink gives a touch: the WinTab pen then does not come near.
        pen.on_ink(&[state((0, 0), (5, 6), 512)], &map);
        pen.handle_msg(HWND0, wintab::WT_PROXIMITY, WPARAM(0), ENTER);
        assert_eq!(take(&events), [touch(5.0, 6.0, 0.5)]);
        // The other windows share the state of the API.
        let (other, other_events) = window(&api);
        other.handle_msg(HWND0, wintab::WT_PROXIMITY, WPARAM(0), ENTER);
        assert_eq!(take(&other_events), []);
    }

    #[test]
    fn the_messages_of_the_api_that_the_app_did_not_pick_give_no_events() {
        let api = Rc::<PenApiState>::default();
        let (pen, events) = window(&api);
        let map = WindowMap::new(None, POINT { x: 0, y: 0 }, 1.0);
        api.set(PenApi::Wintab);
        pen.use_api(HWND0);
        assert_eq!(take(&events), [PenEvent::Leave]);
        pen.handle_msg(HWND0, wintab::WT_PROXIMITY, WPARAM(0), ENTER);
        pen.on_ink(&[state((0, 0), (5, 6), 512)], &map);
        pen.handle_msg(HWND0, WM_POINTERLEAVE, WPARAM(0), OUT);
        assert_eq!(take(&events), [PenEvent::Hover]);
        assert!(!api.ink_touched.get());
        api.set(PenApi::WindowsInk);
        pen.use_api(HWND0);
        pen.handle_msg(HWND0, wintab::WT_PROXIMITY, WPARAM(0), ENTER);
        pen.on_ink(&[state((0, 0), (5, 6), 512)], &map);
        pen.handle_msg(HWND0, wintab::WT_PROXIMITY, WPARAM(0), OUT);
        pen.handle_msg(HWND0, WM_POINTERLEAVE, WPARAM(0), OUT);
        assert_eq!(
            take(&events),
            [PenEvent::Leave, touch(5.0, 6.0, 0.5), PenEvent::Leave]
        );
    }

    #[test]
    fn a_pen_with_no_pressure_paints_at_full_pressure() {
        let map = WindowMap::new(None, POINT { x: 0, y: 0 }, 1.0);
        let mut info = state((0, 0), (5, 6), 1);
        info.penMask = 0;
        assert_eq!(collect(&[info], &map), [touch(5.0, 6.0, 1.0)]);
    }
}
