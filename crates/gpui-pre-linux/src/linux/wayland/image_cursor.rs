//! Cursors made from images. Each image goes into an shm buffer once. A
//! cursor surface shows the buffer at the logical size of the image.
//!
//! The protocols: <https://wayland.app/protocols/wayland#wl_shm> and
//! <https://wayland.app/protocols/viewporter>.

use super::{cursor::CursorTheme, to_shape};
use crate::linux::wayland::client::WaylandClientStatePtr;
use crate::linux::{Globals, cursor_style_to_icon_names};
use anyhow::{Context as _, Result};
use gpui::{CursorImage, CursorImageCache, CursorRequest};
use gpui_util::ResultExt as _;
use std::fs::File;
use std::io::Write as _;
use std::os::fd::{AsFd as _, FromRawFd as _};
use std::sync::Arc;
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_pointer::WlPointer, wl_shm, wl_surface::WlSurface,
};
use wayland_client::{Connection, QueueHandle};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use wayland_protocols::wp::tablet::zv2::client::zwp_tablet_tool_v2::ZwpTabletToolV2;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

/// The buffers of the cursors that the pointer and the tablet tools show.
pub(crate) struct CursorBuffers {
    pub images: ImageCursors,
    pub theme: CursorTheme,
}

impl CursorBuffers {
    pub fn new(connection: &Connection, globals: &Globals, theme_size: u32) -> Self {
        Self {
            images: ImageCursors::new(globals),
            theme: CursorTheme::new(connection, globals, theme_size),
        }
    }
}

/// The shm buffers of the images, by image.
pub(crate) struct ImageCursors {
    shm: wl_shm::WlShm,
    qh: QueueHandle<WaylandClientStatePtr>,

    /// None for an image that has no buffer.
    buffers: CursorImageCache<Option<CursorBuffer>>,
}

/// The buffer of an image. Drop destroys it.
struct CursorBuffer(WlBuffer);

impl Drop for CursorBuffer {
    fn drop(&mut self) {
        self.0.destroy();
    }
}

impl ImageCursors {
    fn new(globals: &Globals) -> Self {
        Self {
            shm: globals.shm.clone(),
            qh: globals.qh.clone(),
            buffers: CursorImageCache::default(),
        }
    }

    /// The buffer of the image. It lives at least as long as the image.
    fn buffer(&mut self, image: &Arc<CursorImage>) -> Option<WlBuffer> {
        let (shm, qh) = (&self.shm, &self.qh);
        self.buffers
            .get_or_insert_with(image, |image| {
                create_buffer(shm, qh, image).log_err().map(CursorBuffer)
            })
            .as_ref()
            .map(|buffer| buffer.0.clone())
    }
}

/// A surface of the cursor role that shows images and theme icons. Each
/// pointer device needs its own, since a surface takes one role.
pub(crate) struct CursorSurface {
    surface: WlSurface,

    /// Scales the buffer to the logical size of the image. Without the
    /// viewporter, a buffer scale does it for images of a whole scale.
    viewport: Option<WpViewport>,
}

impl CursorSurface {
    pub fn new(globals: &Globals) -> Self {
        let surface = globals.compositor.create_surface(&globals.qh, ());
        let viewport = globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &globals.qh, ()));
        Self { surface, viewport }
    }

    /// Shows `cursor` on `target`. Without an image, or when the image fails,
    /// the shape device shows the style. Without a shape device, the theme
    /// icon of the style shows.
    pub fn show_cursor(
        &self,
        cursor: &CursorRequest,
        buffers: &mut CursorBuffers,
        target: CursorTarget<'_>,
    ) {
        if let Some(image) = &cursor.image
            && let Some((x, y)) = self.show(&mut buffers.images, image)
        {
            target.set_surface(&self.surface, x, y);
            return;
        }
        if let Some(shape_device) = target.shape_device {
            shape_device.set_shape(target.serial, to_shape(cursor.style));
            return;
        }
        let scale = (target.scale)();
        let names = cursor_style_to_icon_names(cursor.style);
        let Some(icon) = buffers.theme.icon(names, scale) else {
            return;
        };
        let (x, y) = self.attach(icon, icon.dimensions(), icon.hotspot(), scale as f32);
        target.set_surface(&self.surface, x, y);
    }

    /// Attaches the image to the surface. Gives the hotspot in the logical
    /// pixels of the surface.
    fn show(&self, images: &mut ImageCursors, image: &Arc<CursorImage>) -> Option<(i32, i32)> {
        let buffer = images.buffer(image)?;
        let size = (image.width(), image.height());
        Some(self.attach(&buffer, size, image.hotspot(), image.scale()))
    }

    /// Attaches a buffer of `size` pixels that has `scale` pixels for each
    /// logical pixel. Gives the hotspot in the logical pixels of the surface.
    fn attach(
        &self,
        buffer: &WlBuffer,
        (width, height): (u32, u32),
        (x, y): (u32, u32),
        scale: f32,
    ) -> (i32, i32) {
        let scale = match &self.viewport {
            Some(_) => {
                self.surface.set_buffer_scale(1);
                scale
            }
            None => {
                // A buffer scale must divide the size of the buffer.
                let whole = scale.round() as i32;
                let fits = whole >= 1
                    && (scale - whole as f32).abs() < 0.01
                    && width as i32 % whole == 0
                    && height as i32 % whole == 0;
                let scale = if fits { whole } else { 1 };
                self.surface.set_buffer_scale(scale);
                scale as f32
            }
        };
        let logical = |length: u32| (length as f32 / scale).round() as i32;
        let (logical_width, logical_height) = (logical(width).max(1), logical(height).max(1));
        if let Some(viewport) = &self.viewport {
            viewport.set_destination(logical_width, logical_height);
        }
        self.surface.attach(Some(buffer), 0, 0);
        self.surface.damage(0, 0, logical_width, logical_height);
        self.surface.commit();
        (logical(x), logical(y))
    }
}

/// A device that a cursor surface shows on.
pub(crate) enum CursorDevice<'a> {
    Pointer(&'a WlPointer),
    TabletTool(&'a ZwpTabletToolV2),
}

/// The device that shows a cursor and its state.
pub(crate) struct CursorTarget<'a> {
    pub device: CursorDevice<'a>,
    pub shape_device: Option<&'a WpCursorShapeDeviceV1>,

    /// The serial of the enter event of the device.
    pub serial: u32,

    /// Gives the scale of the output under the device.
    pub scale: &'a dyn Fn() -> i32,
}

impl CursorTarget<'_> {
    /// Gives the device `surface` with the hotspot `(x, y)`.
    fn set_surface(&self, surface: &WlSurface, x: i32, y: i32) {
        match self.device {
            CursorDevice::Pointer(pointer) => pointer.set_cursor(self.serial, Some(surface), x, y),
            CursorDevice::TabletTool(tool) => tool.set_cursor(self.serial, Some(surface), x, y),
        }
    }
}

impl Drop for CursorSurface {
    fn drop(&mut self) {
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        self.surface.destroy();
    }
}

/// An shm buffer with the premultiplied pixels of the image, which is the
/// format of `argb8888`.
fn create_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<WaylandClientStatePtr>,
    image: &CursorImage,
) -> Result<WlBuffer> {
    let pixels = image.premultiplied_bgra();
    let mut file = memory_file()?;
    file.write_all(&pixels)
        .context("Wayland: cannot write a cursor image")?;
    let size = i32::try_from(pixels.len()).context("Wayland: a cursor image is too large")?;
    let pool = shm.create_pool(file.as_fd(), size, qh, ());
    let (width, height) = (image.width() as i32, image.height() as i32);
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    // The buffer keeps the memory of the pool.
    pool.destroy();
    Ok(buffer)
}

/// An anonymous file in memory that the compositor can map.
fn memory_file() -> Result<File> {
    // SAFETY: the name is a valid C string, and the flags are valid.
    let fd = unsafe { libc::memfd_create(c"gpui-cursor".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .context("Wayland: cannot make the memory of a cursor image");
    }
    // SAFETY: the file descriptor is new and has no other owner.
    Ok(unsafe { File::from_raw_fd(fd) })
}
