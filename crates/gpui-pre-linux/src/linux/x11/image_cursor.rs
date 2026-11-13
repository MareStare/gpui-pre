//! Cursors made from images, through the RENDER extension, as x11rb makes
//! the cursors of a theme (`create_render_cursor` of `x11rb::cursor`,
//! <https://github.com/psychon/x11rb/blob/main/x11rb/src/cursor/mod.rs>).
//!
//! The protocol: <https://www.x.org/releases/current/doc/renderproto/renderproto.txt>

use anyhow::{Context as _, Result, anyhow};
use gpui::CursorImage;
use gpui_util::ResultExt as _;
use std::rc::Rc;
use x11rb::connection::Connection as _;
use x11rb::protocol::render::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};
use x11rb::xcb_ffi::XCBConnection;

/// A cursor made from an image. Drop frees it. The server keeps it while
/// a window uses it.
pub(crate) struct ImageCursor {
    connection: Rc<XCBConnection>,
    cursor: xproto::Cursor,
}

impl ImageCursor {
    pub fn cursor(&self) -> xproto::Cursor {
        self.cursor
    }
}

impl Drop for ImageCursor {
    fn drop(&mut self) {
        self.connection.free_cursor(self.cursor).log_err();
    }
}

/// Makes a cursor of the image on the screen of `root`.
pub(crate) fn create(
    connection: &Rc<XCBConnection>,
    root: xproto::Window,
    image: &CursorImage,
) -> Result<ImageCursor> {
    let width = u16::try_from(image.width()).context("X11: a cursor image is too wide")?;
    let height = u16::try_from(image.height()).context("X11: a cursor image is too high")?;
    let format = argb32_format(connection)?;
    let mut pixels = image.premultiplied_bgra();
    // The pixels are 32-bit ARGB values in the byte order of the server.
    if connection.setup().image_byte_order == xproto::ImageOrder::MSB_FIRST {
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.reverse();
        }
    }

    let pixmap = connection.generate_id()?;
    let gc = connection.generate_id()?;
    let picture = connection.generate_id()?;
    let cursor = connection.generate_id()?;
    connection.create_pixmap(32, pixmap, root, width, height)?;
    connection.create_gc(gc, pixmap, &Default::default())?;
    connection.put_image(
        xproto::ImageFormat::Z_PIXMAP,
        pixmap,
        gc,
        width,
        height,
        0,
        0,
        0,
        32,
        &pixels,
    )?;
    connection.render_create_picture(picture, pixmap, format, &Default::default())?;
    let (x, y) = image.hotspot();
    let created = connection
        .render_create_cursor(cursor, picture, x as u16, y as u16)?
        .check();
    connection.render_free_picture(picture)?;
    connection.free_gc(gc)?;
    connection.free_pixmap(pixmap)?;
    created.context("X11: cannot make a cursor of an image")?;
    Ok(ImageCursor {
        connection: connection.clone(),
        cursor,
    })
}

/// The standard picture format of 32-bit ARGB.
fn argb32_format(connection: &XCBConnection) -> Result<render::Pictformat> {
    let formats = connection.render_query_pict_formats()?.reply()?;
    formats
        .formats
        .iter()
        .find(|format| {
            let direct = &format.direct;
            format.type_ == render::PictType::DIRECT
                && format.depth == 32
                && (direct.alpha_shift, direct.alpha_mask) == (24, 0xff)
                && (direct.red_shift, direct.red_mask) == (16, 0xff)
                && (direct.green_shift, direct.green_mask) == (8, 0xff)
                && (direct.blue_shift, direct.blue_mask) == (0, 0xff)
        })
        .map(|format| format.id)
        .ok_or_else(|| anyhow!("X11: the server has no ARGB32 picture format"))
}
