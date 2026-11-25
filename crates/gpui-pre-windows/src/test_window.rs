//! Real windows for the tests of the platform.

use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, Bounds, Empty, GraphicalEnvironment, Platform, PlatformWindow, WindowHandle,
    WindowId, WindowKind, WindowParams, WindowingRequest, point, px, size,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, IsWindow, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW,
    QS_ALLINPUT, TranslateMessage,
};

use crate::WindowsPlatform;

/// A shown window of 400x300 at (100, 100).
pub(crate) fn params() -> WindowParams {
    WindowParams {
        bounds: Bounds::new(point(px(100.0), px(100.0)), size(px(400.0), px(300.0))),
        titlebar: None,
        kind: WindowKind::Normal,
        is_movable: true,
        app_owns_titlebar_drag: false,
        is_resizable: true,
        is_minimizable: true,
        focus: true,
        show: true,
        icon: None,
        display_id: None,
        app_id: None,
        window_min_size: None,
    }
}

/// Starts a platform with windowing and opens a window on it.
pub(crate) fn open(params: WindowParams) -> (WindowsPlatform, Box<dyn PlatformWindow>) {
    let platform = WindowsPlatform::new(false).unwrap();
    let windowed =
        platform.request_windowing(WindowingRequest::Windowed(GraphicalEnvironment::detect()));
    futures::executor::block_on(windowed).unwrap();
    let handle: AnyWindowHandle = WindowHandle::<Empty>::new(WindowId::from(1)).into();
    let window = platform.open_window(handle, params).unwrap();
    (platform, window)
}

/// Drops the window and waits until the system destroyed it.
pub(crate) fn close(window: Box<dyn PlatformWindow>) {
    let hwnd = window.get_raw_handle();
    drop(window);
    assert!(
        pump_until(|| !unsafe { IsWindow(Some(hwnd)) }.as_bool()),
        "the window stays after its drop"
    );
}

/// Handles the messages of the thread until `done` holds, for at most 5 seconds. Returns
/// whether it holds.
pub(crate) fn pump_until(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if done() {
            return true;
        }
        if Instant::now() > deadline {
            return false;
        }
        unsafe { MsgWaitForMultipleObjects(None, false, 10, QS_ALLINPUT) };
    }
}
