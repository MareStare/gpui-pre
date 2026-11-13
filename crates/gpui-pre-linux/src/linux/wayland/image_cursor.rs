//! Cursors made from images. Each image goes into an shm buffer once. A
//! cursor surface shows the buffer at the logical size of the image.
//!
//! The protocols: <https://wayland.app/protocols/wayland#wl_shm> and
//! <https://wayland.app/protocols/viewporter>.

use crate::linux::Globals;
use crate::linux::wayland::client::WaylandClientStatePtr;
use anyhow::{Context as _, Result};
use collections::HashMap;
use gpui::{CursorImage, CursorImageId};
use std::fs::File;
use std::io::Write as _;
use std::os::fd::{AsFd as _, FromRawFd as _};
use wayland_client::QueueHandle;
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_shm, wl_surface::WlSurface};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

/// The shm buffers of the images, by image.
#[derive(Default)]
pub(crate) struct ImageCursors {
    buffers: HashMap<CursorImageId, WlBuffer>,
}

impl ImageCursors {
    /// The buffer of the image. It lives until the connection ends.
    fn buffer(&mut self, globals: &Globals, image: &CursorImage) -> Result<WlBuffer> {
        if let Some(buffer) = self.buffers.get(&image.id()) {
            return Ok(buffer.clone());
        }
        let buffer = create_buffer(&globals.shm, &globals.qh, image)?;
        self.buffers.insert(image.id(), buffer.clone());
        Ok(buffer)
    }
}

impl Drop for ImageCursors {
    fn drop(&mut self) {
        for buffer in self.buffers.values() {
            buffer.destroy();
        }
    }
}

/// A surface of the cursor role that shows images. Each pointer device
/// needs its own, since a surface takes one role.
pub(crate) struct ImageCursorSurface {
    surface: WlSurface,

    /// Scales the buffer to the logical size of the image. Without the
    /// viewporter, a buffer scale does it for images of a whole scale.
    viewport: Option<WpViewport>,
}

impl ImageCursorSurface {
    pub fn new(globals: &Globals) -> Self {
        let surface = globals.compositor.create_surface(&globals.qh, ());
        let viewport = globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &globals.qh, ()));
        Self { surface, viewport }
    }

    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Attaches the image to the surface. Gives the hotspot in the logical
    /// pixels of the surface, for `set_cursor` of the device.
    pub fn show(
        &self,
        cursors: &mut ImageCursors,
        globals: &Globals,
        image: &CursorImage,
    ) -> Result<(i32, i32)> {
        let buffer = cursors.buffer(globals, image)?;
        let (width, height) = (image.width() as i32, image.height() as i32);
        let scale = image.scale();
        let logical = |length: u32| (length as f32 / scale).round() as i32;
        match &self.viewport {
            Some(viewport) => {
                self.surface.set_buffer_scale(1);
                viewport.set_destination(
                    logical(image.width()).max(1),
                    logical(image.height()).max(1),
                );
            }
            None => {
                // A buffer scale must divide the size of the buffer.
                let whole = scale.round() as i32;
                let fits = whole >= 1
                    && (scale - whole as f32).abs() < 0.01
                    && width % whole == 0
                    && height % whole == 0;
                self.surface.set_buffer_scale(if fits { whole } else { 1 });
            }
        }
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, width, height);
        self.surface.commit();
        let (x, y) = image.hotspot();
        Ok((logical(x), logical(y)))
    }
}

impl Drop for ImageCursorSurface {
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
