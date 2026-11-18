use crate::linux::Globals;
use crate::linux::{DEFAULT_CURSOR_ICON_NAME, log_cursor_icon_warning};
use anyhow::{Context as _, anyhow};
use gpui_util::ResultExt;

use wayland_client::Connection;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_cursor::{CursorImageBuffer, CursorTheme};

/// The cursor theme. It gives the buffer of each cursor icon.
pub(crate) struct Cursor {
    loaded_theme: Option<LoadedTheme>,
    size: u32,
    scaled_size: u32,
    shm: WlShm,
    connection: Connection,
}

pub(crate) struct LoadedTheme {
    theme: CursorTheme,
    name: Option<String>,
    scaled_size: u32,
}

impl Cursor {
    pub fn new(connection: &Connection, globals: &Globals, size: u32) -> Self {
        let mut this = Self {
            loaded_theme: None,
            size,
            scaled_size: size,
            shm: globals.shm.clone(),
            connection: connection.clone(),
        };
        this.set_theme_internal(None);
        this
    }

    fn set_theme_internal(&mut self, theme_name: Option<String>) {
        if let Some(loaded_theme) = self.loaded_theme.as_ref()
            && loaded_theme.name == theme_name
            && loaded_theme.scaled_size == self.scaled_size
        {
            return;
        }
        let result = if let Some(theme_name) = theme_name.as_ref() {
            CursorTheme::load_from_name(
                &self.connection,
                self.shm.clone(),
                theme_name,
                self.scaled_size,
            )
        } else {
            CursorTheme::load(&self.connection, self.shm.clone(), self.scaled_size)
        };
        if let Some(theme) = result
            .context("Wayland: Failed to load cursor theme")
            .log_err()
        {
            self.loaded_theme = Some(LoadedTheme {
                theme,
                name: theme_name,
                scaled_size: self.scaled_size,
            });
        }
    }

    pub fn set_theme(&mut self, theme_name: String) {
        self.set_theme_internal(Some(theme_name));
    }

    fn set_scaled_size(&mut self, scaled_size: u32) {
        self.scaled_size = scaled_size;
        let theme_name = self
            .loaded_theme
            .as_ref()
            .and_then(|loaded_theme| loaded_theme.name.clone());
        self.set_theme_internal(theme_name);
    }

    pub fn set_size(&mut self, size: u32) {
        self.size = size;
        self.set_scaled_size(size);
    }

    /// The buffer of the first icon of `cursor_icon_names` in the theme,
    /// or of the default icon, for a buffer scale of `scale`.
    pub fn icon(&mut self, cursor_icon_names: &[&str], scale: i32) -> Option<&CursorImageBuffer> {
        self.set_scaled_size(self.size * scale as u32);

        let Some(loaded_theme) = &mut self.loaded_theme else {
            log::warn!("Wayland: Unable to load cursor themes");
            return None;
        };
        let theme = &mut loaded_theme.theme;

        let found = cursor_icon_names
            .iter()
            .copied()
            .find(|name| theme.get_cursor(name).is_some());
        let name = match found {
            Some(name) => name,
            None if theme.get_cursor(DEFAULT_CURSOR_ICON_NAME).is_some() => {
                log_cursor_icon_warning(anyhow!(
                    "wayland: Unable to get cursor icon {:?}. \
                    Using default cursor icon: '{}'",
                    cursor_icon_names,
                    DEFAULT_CURSOR_ICON_NAME
                ));
                DEFAULT_CURSOR_ICON_NAME
            }
            None => {
                log_cursor_icon_warning(anyhow!(
                    "wayland: Unable to fallback on default cursor icon '{}' for theme '{}'",
                    DEFAULT_CURSOR_ICON_NAME,
                    loaded_theme.name.as_deref().unwrap_or("default")
                ));
                return None;
            }
        };
        theme.get_cursor(name).map(|cursor| &cursor[0])
    }
}
