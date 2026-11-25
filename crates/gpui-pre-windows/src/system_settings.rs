use std::{
    cell::Cell,
    ffi::{c_uint, c_void},
};

use gpui_util::ResultExt;
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETWHEELSCROLLCHARS, SPI_GETWHEELSCROLLLINES, SPI_SETWHEELSCROLLCHARS,
    SPI_SETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_ACTION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SystemParametersInfoW,
};

/// Windows settings pulled from SystemParametersInfo
/// https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-systemparametersinfow
#[derive(Default, Debug, Clone)]
pub(crate) struct WindowsSystemSettings {
    pub(crate) mouse_wheel_settings: MouseWheelSettings,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct MouseWheelSettings {
    /// SEE: SPI_GETWHEELSCROLLCHARS
    pub(crate) wheel_scroll_chars: Cell<u32>,
    /// SEE: SPI_GETWHEELSCROLLLINES
    pub(crate) wheel_scroll_lines: Cell<u32>,
}

impl WindowsSystemSettings {
    pub(crate) fn new() -> Self {
        let mut settings = Self::default();
        settings.init();
        settings
    }

    fn init(&mut self) {
        self.mouse_wheel_settings.update();
    }

    /// Takes the `wparam` of `WM_SETTINGCHANGE`: the `SPI_SET*` action that changed a setting.
    /// https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-settingchange
    pub(crate) fn update(&self, wparam: usize) {
        if changes_mouse_wheel_settings(wparam) {
            self.update_mouse_wheel_settings();
        }
    }

    fn update_mouse_wheel_settings(&self) {
        self.mouse_wheel_settings.update();
    }
}

fn changes_mouse_wheel_settings(wparam: usize) -> bool {
    matches!(
        SYSTEM_PARAMETERS_INFO_ACTION(wparam as u32),
        SPI_SETWHEELSCROLLLINES | SPI_SETWHEELSCROLLCHARS
    )
}

impl MouseWheelSettings {
    fn update(&self) {
        self.update_wheel_scroll_chars();
        self.update_wheel_scroll_lines();
    }

    fn update_wheel_scroll_chars(&self) {
        let mut value = c_uint::default();
        let result = unsafe {
            SystemParametersInfoW(
                SPI_GETWHEELSCROLLCHARS,
                0,
                Some((&mut value) as *mut c_uint as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
            )
        };

        if result.log_err() != None && self.wheel_scroll_chars.get() != value {
            self.wheel_scroll_chars.set(value);
        }
    }

    fn update_wheel_scroll_lines(&self) {
        let mut value = c_uint::default();
        let result = unsafe {
            SystemParametersInfoW(
                SPI_GETWHEELSCROLLLINES,
                0,
                Some((&mut value) as *mut c_uint as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS::default(),
            )
        };

        if result.log_err() != None && self.wheel_scroll_lines.get() != value {
            self.wheel_scroll_lines.set(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_scroll_set_actions_change_mouse_wheel_settings() {
        assert!(changes_mouse_wheel_settings(SPI_SETWHEELSCROLLLINES.0 as usize));
        assert!(changes_mouse_wheel_settings(SPI_SETWHEELSCROLLCHARS.0 as usize));
        assert!(!changes_mouse_wheel_settings(SPI_GETWHEELSCROLLLINES.0 as usize));
        assert!(!changes_mouse_wheel_settings(SPI_GETWHEELSCROLLCHARS.0 as usize));
    }
}
