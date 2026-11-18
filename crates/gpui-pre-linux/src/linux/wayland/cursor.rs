use crate::linux::Globals;
use crate::linux::{DEFAULT_CURSOR_ICON_NAME, log_cursor_icon_warning};
use anyhow::{Context as _, anyhow};
use collections::HashMap;
use gpui_util::ResultExt;

use wayland_client::Connection;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_cursor::CursorImageBuffer;

/// The cursor theme. It gives the buffer of each cursor icon.
pub(crate) struct CursorTheme {
    /// None for the default theme.
    name: Option<String>,
    size: u32,

    /// The theme of each scaled size that an icon was asked for. None for a size
    /// that failed to load.
    loaded: HashMap<u32, Option<wayland_cursor::CursorTheme>>,
    shm: WlShm,
    connection: Connection,
}

impl CursorTheme {
    pub fn new(connection: &Connection, globals: &Globals, size: u32) -> Self {
        Self {
            name: None,
            size,
            loaded: HashMap::default(),
            shm: globals.shm.clone(),
            connection: connection.clone(),
        }
    }

    pub fn set_theme(&mut self, name: String) {
        if self.name.as_ref() != Some(&name) {
            self.name = Some(name);
            self.loaded.clear();
        }
    }

    pub fn set_size(&mut self, size: u32) {
        if self.size != size {
            self.size = size;
            self.loaded.clear();
        }
    }

    /// The buffer of the first icon of `cursor_icon_names` in the theme,
    /// or of the default icon, for a buffer scale of `scale`.
    pub fn icon(&mut self, cursor_icon_names: &[&str], scale: i32) -> Option<&CursorImageBuffer> {
        let scaled_size = self.size * scale as u32;
        let (connection, shm, theme_name) = (&self.connection, &self.shm, &self.name);
        let theme = self
            .loaded
            .entry(scaled_size)
            .or_insert_with(|| load_theme(connection, shm, theme_name.as_deref(), scaled_size))
            .as_mut()?;

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
                    self.name.as_deref().unwrap_or("default")
                ));
                return None;
            }
        };
        theme.get_cursor(name).map(|cursor| &cursor[0])
    }
}

/// Loads the theme `name`, or the default theme, with icons of `size` pixels.
fn load_theme(
    connection: &Connection,
    shm: &WlShm,
    name: Option<&str>,
    size: u32,
) -> Option<wayland_cursor::CursorTheme> {
    let result = match name {
        Some(name) => {
            wayland_cursor::CursorTheme::load_from_name(connection, shm.clone(), name, size)
        }
        None => wayland_cursor::CursorTheme::load(connection, shm.clone(), size),
    };
    result
        .context("Wayland: Failed to load cursor theme")
        .log_err()
}
