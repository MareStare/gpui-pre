//! The WinTab API of the tablet driver. The driver installs
//! `wintab32.dll` in `System32`. The specification is at
//! <https://developer-docs.wacom.com/docs/icbt/windows/wintab/wintab-reference/>.
//!
//! The context gives the packets in the units of the tablet. This module
//! maps them to the screen, as the specification describes. Some drivers
//! map the tablet to another area than the cursor, so a packet far from the
//! cursor gives its place to the cursor.
//!
//! A driver can stop to answer WinTab calls. A thread other than the UI
//! thread opens each context, so such a driver blocks only that thread.

use std::{
    cell::RefCell,
    ffi::c_void,
    ptr::NonNull,
    sync::{
        OnceLock,
        mpsc::{self, Receiver, TryRecvError},
    },
};

use gpui::PenEvent;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, WPARAM},
        System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        UI::WindowsAndMessaging::{GetCursorPos, PostMessageW},
    },
    core::{s, w},
};

use crate::{SafeHwnd, WM_GPUI_WINTAB_OPENED, pen::WindowMap};

type Context = *mut c_void;
type RawFn = unsafe extern "system" fn() -> isize;

const WTI_DEFSYSCTX: u32 = 4;
const WTI_DEVICES: u32 = 100;
const DVC_NPRESSURE: u32 = 15;
const CXO_MESSAGES: u32 = 0x0004;
const PK_X: u32 = 0x0080;
const PK_Y: u32 = 0x0100;
const PK_NORMAL_PRESSURE: u32 = 0x0400;
const WT_DEFBASE: u32 = 0x7ff0;
pub(crate) const WT_PACKET: u32 = WT_DEFBASE;
pub(crate) const WT_PROXIMITY: u32 = WT_DEFBASE + 5;

/// The `LOGCONTEXTW` struct.
#[repr(C)]
struct LogContext {
    name: [u16; 40],
    options: u32,
    status: u32,
    locks: u32,
    msg_base: u32,
    device: u32,
    pkt_rate: u32,
    pkt_data: u32,
    pkt_mode: u32,
    move_mask: u32,
    btn_dn_mask: u32,
    btn_up_mask: u32,
    in_org: [i32; 3],
    in_ext: [i32; 3],
    out_org: [i32; 3],
    out_ext: [i32; 3],
    sens: [u32; 3],
    sys_mode: i32,
    sys_org: [i32; 2],
    sys_ext: [i32; 2],
    sys_sens: [u32; 2],
}

const _: () = assert!(
    size_of::<LogContext>() == 212,
    "`LOGCONTEXTW` has 212 bytes"
);

/// The `AXIS` struct.
#[repr(C)]
#[derive(Default)]
struct Axis {
    min: i32,
    max: i32,
    units: u32,
    resolution: u32,
}

type WtInfo = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
type WtOpen = unsafe extern "system" fn(HWND, *mut LogContext, i32) -> Context;
type WtClose = unsafe extern "system" fn(Context) -> i32;
type WtPacketsGet = unsafe extern "system" fn(Context, i32, *mut c_void) -> i32;
type WtQueueSizeSet = unsafe extern "system" fn(Context, i32) -> i32;
type WtOverlap = unsafe extern "system" fn(Context, i32) -> i32;

pub(crate) struct Api {
    info: WtInfo,
    open: WtOpen,
    close: WtClose,
    packets_get: WtPacketsGet,
    queue_size_set: WtQueueSizeSet,
    overlap: WtOverlap,
}

/// Gives the WinTab API, or `None` when no driver installed it.
pub(crate) type Loader = fn() -> Option<&'static Api>;

/// The packets that the context keeps. A fast pen gives several packets
/// before the window reads them.
const QUEUE_SIZE: i32 = 128;

/// A packet of the fields `PK_X`, `PK_Y` and `PK_NORMAL_PRESSURE`. The
/// fields come in the order of their bits.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Packet {
    x: i32,
    y: i32,
    pressure: u32,
}

/// Maps the packets of the tablet to the screen.
#[derive(Clone, Copy, Debug)]
struct Scale {
    /// The output extent of the context. The packet values span it.
    out_ext: [i32; 2],

    /// The area of the screen that the tablet maps to, in physical pixels.
    sys_org: [i32; 2],
    sys_ext: [i32; 2],
    max_pressure: u32,
}

impl Scale {
    /// A packet is in another area than the cursor when it is farther than
    /// this from the cursor, in screen pixels.
    const CURSOR_DISTANCE: f64 = 64.0;

    fn to_screen(self, packet: Packet) -> (f64, f64) {
        let axis = |value: i32, out_ext: i32, sys_org: i32, sys_ext: i32| {
            let k = f64::from(sys_ext) / f64::from(out_ext.unsigned_abs().max(1));
            f64::from(value) * k + f64::from(sys_org)
        };
        let ([ox, oy], [sx, sy], [ex, ey]) = (self.out_ext, self.sys_org, self.sys_ext);
        (axis(packet.x, ox, sx, ex), axis(packet.y, oy, sy, ey))
    }

    fn pressure(self, packet: Packet) -> f32 {
        let max = self.max_pressure.max(1);
        packet.pressure.min(max) as f32 / max as f32
    }

    /// Gives the pen events of the packets, oldest first. When the newest
    /// packet is far from the `cursor`, the packets give one event at the
    /// cursor.
    fn events(
        self,
        packets: &[Packet],
        map: &WindowMap,
        cursor: Option<(f64, f64)>,
        emit: &mut dyn FnMut(PenEvent),
    ) {
        let Some(&newest) = packets.last() else {
            return;
        };
        let (x, y) = self.to_screen(newest);
        let elsewhere = cursor.filter(|&(cx, cy)| {
            (x - cx).abs() > Self::CURSOR_DISTANCE || (y - cy).abs() > Self::CURSOR_DISTANCE
        });
        let samples = elsewhere.map_or_else(
            || packets.iter().map(|&p| (self.to_screen(p), p)).collect(),
            |cursor| vec![(cursor, newest)],
        );
        for (screen, packet) in samples {
            if packet.pressure == 0 {
                emit(PenEvent::Hover);
            } else {
                emit(PenEvent::Touch {
                    position: map.screen_to_window(screen),
                    pressure: self.pressure(packet),
                });
            }
        }
    }
}

/// Loads the driver library once. The library stays loaded.
pub(crate) fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(load).as_ref()
}

fn load() -> Option<Api> {
    // The search is only in `System32`, so a file in the working directory
    // cannot replace the library.
    let lib =
        unsafe { LoadLibraryExW(w!("wintab32.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }.ok()?;
    let get = |name| unsafe { GetProcAddress(lib, name) };
    // The function types agree with the WinTab specification.
    unsafe {
        Some(Api {
            info: std::mem::transmute::<RawFn, WtInfo>(get(s!("WTInfoW"))?),
            open: std::mem::transmute::<RawFn, WtOpen>(get(s!("WTOpenW"))?),
            close: std::mem::transmute::<RawFn, WtClose>(get(s!("WTClose"))?),
            packets_get: std::mem::transmute::<RawFn, WtPacketsGet>(get(s!("WTPacketsGet"))?),
            queue_size_set: std::mem::transmute::<RawFn, WtQueueSizeSet>(get(s!(
                "WTQueueSizeSet"
            ))?),
            overlap: std::mem::transmute::<RawFn, WtOverlap>(get(s!("WTOverlap"))?),
        })
    }
}

/// An open context of the tablet. Dropping it closes the context.
struct Tablet {
    api: &'static Api,
    context: NonNull<c_void>,
    scale: Scale,
}

// The context handle names a context of the process, not of a thread. The
// thread that opens the context gives it to the UI thread.
unsafe impl Send for Tablet {}

impl Tablet {
    /// Opens a context that sends packets to the window. Returns `None` when
    /// the driver gives no context. The driver can take any time to answer,
    /// or never answer.
    fn open(api: &'static Api, hwnd: HWND) -> Option<Self> {
        // All fields are integers, so zero is a valid value.
        let mut ctx: LogContext = unsafe { std::mem::zeroed() };
        let mut axis = Axis::default();
        // The out pointers are valid for the types that the categories write.
        unsafe {
            if (api.info)(WTI_DEFSYSCTX, 0, (&raw mut ctx).cast()) == 0 {
                return None;
            }
            (api.info)(
                WTI_DEVICES + ctx.device,
                DVC_NPRESSURE,
                (&raw mut axis).cast(),
            );
        }
        let max = u32::try_from(axis.max).unwrap_or(0);
        ctx.options |= CXO_MESSAGES;
        ctx.msg_base = WT_DEFBASE;
        ctx.pkt_data = PK_X | PK_Y | PK_NORMAL_PRESSURE;
        ctx.pkt_mode = 0;
        ctx.move_mask = PK_X | PK_Y | PK_NORMAL_PRESSURE;
        // The packets span the input extent. A negative extent puts the
        // origin at the top.
        ctx.out_org = [0; 3];
        ctx.out_ext = [ctx.in_ext[0], -ctx.in_ext[1], ctx.in_ext[2]];
        let scale = Scale {
            out_ext: [ctx.out_ext[0], ctx.out_ext[1]],
            sys_org: ctx.sys_org,
            sys_ext: ctx.sys_ext,
            max_pressure: if max == 0 { 1023 } else { max },
        };
        let context = NonNull::new(unsafe { (api.open)(hwnd, &raw mut ctx, 1) })?;
        // A failed call keeps the queue of the driver.
        unsafe { (api.queue_size_set)(context.as_ptr(), QUEUE_SIZE) };
        Some(Self {
            api,
            context,
            scale,
        })
    }

    /// Takes the packets that wait in the context and gives their events.
    fn read_packets(&self, hwnd: HWND, emit: &mut dyn FnMut(PenEvent)) {
        let mut packets = [Packet::default(); QUEUE_SIZE as usize];
        // The buffer has room for the packets.
        let count = unsafe {
            (self.api.packets_get)(
                self.context.as_ptr(),
                QUEUE_SIZE,
                packets.as_mut_ptr().cast(),
            )
        };
        let count = usize::try_from(count).unwrap_or(0).min(packets.len());
        let mut cursor = POINT { x: 0, y: 0 };
        let cursor = unsafe { GetCursorPos(&raw mut cursor) }
            .is_ok()
            .then(|| (f64::from(cursor.x), f64::from(cursor.y)));
        let map = WindowMap::read(hwnd, None);
        let packets = packets.get(..count).unwrap_or_default();
        self.scale.events(packets, &map, cursor, emit);
    }

    /// Puts the context on top of the other contexts. Tablets send
    /// packets only to the top context.
    fn bring_to_top(&self) {
        unsafe { (self.api.overlap)(self.context.as_ptr(), 1) };
    }
}

impl Drop for Tablet {
    fn drop(&mut self) {
        // Only this drop closes the context.
        unsafe { (self.api.close)(self.context.as_ptr()) };
    }
}

/// The WinTab context of a window.
enum Link {
    /// The pen API reads no WinTab, or the driver gave no context.
    Closed,

    /// A thread opens the context and posts `WM_GPUI_WINTAB_OPENED` when
    /// the driver answers. Each WinTab call can block until then, so the UI
    /// thread makes none.
    Opening(Receiver<Tablet>),
    Open(Tablet),
}

/// The WinTab context of one window.
pub(crate) struct Wintab {
    link: RefCell<Link>,
    load: Loader,
}

impl Wintab {
    pub(crate) fn new(load: Loader) -> Self {
        Self {
            link: RefCell::new(Link::Closed),
            load,
        }
    }

    /// Opens the context when `wanted` is true, and closes it otherwise.
    pub(crate) fn want(&self, hwnd: HWND, wanted: bool) {
        self.relink(|link| match (wanted, link) {
            (true, Link::Closed) => Some(self.start_open(hwnd)),
            (false, Link::Opening(_) | Link::Open(_)) => Some(Link::Closed),
            (true, Link::Opening(_) | Link::Open(_)) | (false, Link::Closed) => None,
        });
    }

    /// Answers `WM_GPUI_WINTAB_OPENED`.
    pub(crate) fn on_opened(&self) {
        self.relink(|link| match link {
            Link::Opening(receive) => match receive.try_recv() {
                Ok(tablet) => Some(Link::Open(tablet)),
                Err(TryRecvError::Disconnected) => Some(Link::Closed),
                Err(TryRecvError::Empty) => None,
            },
            Link::Closed | Link::Open(_) => None,
        });
    }

    /// Answers `WT_PACKET`. The message of a context that closed reads
    /// nothing.
    pub(crate) fn read_packets(&self, hwnd: HWND, lparam: LPARAM, emit: &mut dyn FnMut(PenEvent)) {
        if let Link::Open(tablet) = &*self.link.borrow()
            && tablet.context.as_ptr() as isize == lparam.0
        {
            tablet.read_packets(hwnd, emit);
        }
    }

    /// Puts the context of the window on top of the other contexts.
    pub(crate) fn bring_to_top(&self) {
        if let Link::Open(tablet) = &*self.link.borrow() {
            tablet.bring_to_top();
        }
    }

    /// Sets the link to the link that `next` gives. The old link closes
    /// outside of the borrow: the driver can send a message to the window
    /// when it closes a context.
    fn relink(&self, next: impl FnOnce(&Link) -> Option<Link>) {
        let new = next(&self.link.borrow());
        let old = new.map(|new| self.link.replace(new));
        drop(old);
    }

    /// Starts a thread that opens the context of the window.
    fn start_open(&self, hwnd: HWND) -> Link {
        let load = self.load;
        let (send, receive) = mpsc::channel();
        let hwnd = SafeHwnd::from(hwnd);
        let thread = std::thread::Builder::new()
            .name("WinTab open".into())
            .spawn(move || {
                let tablet = load().and_then(|api| Tablet::open(api, hwnd.as_raw()));
                // The window dropped its link when the send fails. The tablet
                // closes here then.
                if let Some(tablet) = tablet {
                    send.send(tablet).ok();
                }
                drop(send);
                // A window that went away gives an error.
                unsafe {
                    PostMessageW(
                        Some(hwnd.as_raw()),
                        WM_GPUI_WINTAB_OPENED,
                        WPARAM(0),
                        LPARAM(0),
                    )
                }
                .ok();
            });
        match thread {
            Ok(_) => Link::Opening(receive),
            Err(error) => {
                log::warn!("cannot start the thread that opens WinTab: {error}");
                Link::Closed
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    /// A tablet of 10000 by 6000 units on a screen of 2000 by 1200 pixels:
    /// 0.2 pixel per unit.
    const SCALE: Scale = Scale {
        out_ext: [10000, -6000],
        sys_org: [0, 0],
        sys_ext: [2000, 1200],
        max_pressure: 1000,
    };

    /// The client area starts at (100, 40), and the display has a scale of
    /// 125%.
    fn map() -> WindowMap {
        WindowMap::new(None, POINT { x: 100, y: 40 }, 1.25)
    }

    const fn packet(x: i32, y: i32, pressure: u32) -> Packet {
        Packet { x, y, pressure }
    }

    fn events(packets: &[Packet], cursor: Option<(f64, f64)>) -> Vec<PenEvent> {
        let mut events = Vec::new();
        SCALE.events(packets, &map(), cursor, &mut |event| events.push(event));
        events
    }

    #[test]
    fn the_packets_give_every_sample_in_order() {
        let packets = (0..20i32)
            .map(|i| packet(1003 + i * 37, 2001 + i * i, 50 * i as u32))
            .collect::<Vec<_>>();
        let newest = SCALE.to_screen(*packets.last().unwrap());
        let events = events(&packets, Some(newest));
        // The first packet has no pressure: the pen does not touch yet.
        assert_eq!(events.first(), Some(&PenEvent::Hover));
        assert_eq!(events.len(), 20);
        for (event, packet) in events.iter().zip(&packets).skip(1) {
            let PenEvent::Touch { position, pressure } = *event else {
                panic!("{event:?}");
            };
            let x = (f64::from(packet.x) / 5.0 - 100.0) / 1.25;
            let y = (f64::from(packet.y) / 5.0 - 40.0) / 1.25;
            assert!(
                (f64::from(position.x.as_f32()) - x).abs() < 1e-3,
                "{event:?} {x}"
            );
            assert!(
                (f64::from(position.y.as_f32()) - y).abs() < 1e-3,
                "{event:?} {y}"
            );
            assert_eq!(pressure, packet.pressure as f32 / 1000.0);
        }
        assert!(events.iter().any(|event| matches!(
            event,
            PenEvent::Touch { position, .. } if position.x.as_f32().fract() != 0.0
        )));
    }

    #[test]
    fn a_packet_far_from_the_cursor_gives_its_place_to_the_cursor() {
        let packets = [packet(0, 0, 500), packet(10, 10, 600)];
        let touch = PenEvent::Touch {
            position: point(px(200.0), px(80.0)),
            pressure: 0.6,
        };
        assert_eq!(events(&packets, Some((350.0, 140.0))), [touch]);
        assert_eq!(events(&[packet(0, 0, 0)], None), [PenEvent::Hover]);
    }

    /// A driver that answers `WTOpen` only after `answer`.
    mod slow_driver {
        use super::super::{Api, Context, LogContext};
        use parking_lot::{Condvar, Mutex};
        use std::ffi::c_void;
        use std::ptr::NonNull;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use windows::Win32::Foundation::HWND;

        static ANSWERS: Mutex<bool> = Mutex::new(false);
        static ANSWERED: Condvar = Condvar::new();
        static CLOSED: AtomicUsize = AtomicUsize::new(0);

        unsafe extern "system" fn info(_: u32, _: u32, _: *mut c_void) -> u32 {
            1
        }

        unsafe extern "system" fn open(_: HWND, _: *mut LogContext, _: i32) -> Context {
            let mut answers = ANSWERS.lock();
            ANSWERED.wait_while(&mut answers, |answers| !*answers);
            NonNull::dangling().as_ptr()
        }

        unsafe extern "system" fn close(_: Context) -> i32 {
            CLOSED.fetch_add(1, Ordering::SeqCst);
            1
        }

        unsafe extern "system" fn packets_get(_: Context, _: i32, _: *mut c_void) -> i32 {
            0
        }

        unsafe extern "system" fn set(_: Context, _: i32) -> i32 {
            1
        }

        static API: Api = Api {
            info,
            open,
            close,
            packets_get,
            queue_size_set: set,
            overlap: set,
        };

        pub(super) fn load() -> Option<&'static Api> {
            Some(&API)
        }

        pub(super) fn answer() {
            *ANSWERS.lock() = true;
            ANSWERED.notify_all();
        }

        pub(super) fn closed() -> usize {
            CLOSED.load(Ordering::SeqCst)
        }
    }

    fn is_open(wintab: &Wintab) -> bool {
        matches!(*wintab.link.borrow(), Link::Open(_))
    }

    fn wait_until(done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "no answer");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn a_driver_that_does_not_answer_blocks_no_window() {
        let hwnd = HWND(std::ptr::null_mut());
        // The window goes away before the driver answers.
        let gone = Wintab::new(slow_driver::load);
        gone.want(hwnd, true);
        drop(gone);
        let wintab = Wintab::new(slow_driver::load);
        wintab.want(hwnd, true);
        assert!(!is_open(&wintab));
        slow_driver::answer();
        wait_until(|| {
            wintab.on_opened();
            is_open(&wintab)
        });
        // The context of the window that went away closes in its thread.
        wait_until(|| slow_driver::closed() == 1);
        wintab.want(hwnd, false);
        assert_eq!(slow_driver::closed(), 2);
        assert!(!is_open(&wintab));
    }
}
