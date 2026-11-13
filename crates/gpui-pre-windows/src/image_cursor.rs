//! Cursors made from images: a color bitmap with an alpha channel and a
//! mask bitmap.
//!
//! The cursor format: <https://learn.microsoft.com/en-us/windows/win32/menurc/using-cursors>

use anyhow::{Context as _, Result};
use gpui::{CursorImage, CursorImageCache};
use gpui_util::ResultExt as _;
use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::Arc;
use windows::Win32::Graphics::Gdi::{
    BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyCursor, HCURSOR, ICONINFO,
};

thread_local! {
    /// The cursors of the images. None for an image that the system did
    /// not take.
    static CURSORS: RefCell<CursorImageCache<Option<OwnedCursor>>> = RefCell::default();
}

/// A cursor that the app made. Drop destroys it.
struct OwnedCursor(HCURSOR);

impl Drop for OwnedCursor {
    fn drop(&mut self) {
        // SAFETY: the app made the cursor, and nothing uses it after the drop.
        unsafe { DestroyCursor(self.0) }.log_err();
    }
}

/// The cursor of the image. It lives at least as long as the image.
pub(crate) fn cursor(image: &Arc<CursorImage>) -> Option<HCURSOR> {
    CURSORS.with_borrow_mut(|cursors| {
        cursors
            .get_or_insert_with(image, |image| create(image).log_err().map(OwnedCursor))
            .as_ref()
            .map(|cursor| cursor.0)
    })
}

fn create(image: &CursorImage) -> Result<HCURSOR> {
    let width = i32::try_from(image.width()).context("a cursor image is too wide")?;
    let height = i32::try_from(image.height()).context("a cursor image is too high")?;
    let header = BITMAPV5HEADER {
        bV5Size: size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: width,
        // A negative height puts the top row first.
        bV5Height: -height,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5RedMask: 0x00ff_0000,
        bV5GreenMask: 0x0000_ff00,
        bV5BlueMask: 0x0000_00ff,
        bV5AlphaMask: 0xff00_0000,
        ..Default::default()
    };
    let mask_bits = and_mask(image);
    let mut bits = null_mut();
    // SAFETY: the header and the out pointer are valid for the calls. The
    // bitmap has `width * height` pixels of 4 bytes, as many as the image.
    // The mask has whole 16-bit words for each of its rows.
    unsafe {
        let color = CreateDIBSection(
            None,
            (&raw const header).cast::<BITMAPINFO>(),
            DIB_RGB_COLORS,
            &raw mut bits,
            None,
            0,
        )
        .context("cannot make the bitmap of a cursor")?;
        anyhow::ensure!(!bits.is_null(), "a cursor bitmap has no pixels");
        std::ptr::copy_nonoverlapping(image.bgra().as_ptr(), bits.cast::<u8>(), image.bgra().len());
        let mask = CreateBitmap(width, height, 1, 1, Some(mask_bits.as_ptr().cast()));
        if mask.is_invalid() {
            let _ = DeleteObject(color.into());
            anyhow::bail!("cannot make the mask of a cursor");
        }
        let info = ICONINFO {
            fIcon: false.into(),
            xHotspot: image.hotspot().0,
            yHotspot: image.hotspot().1,
            hbmMask: mask,
            hbmColor: color,
        };
        let cursor = CreateIconIndirect(&raw const info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        Ok(HCURSOR(
            cursor.context("cannot make a cursor of an image")?.0,
        ))
    }
}

/// The AND mask of the cursor: a set bit shows the screen. The system
/// ignores the alpha channel when all alpha values are 0, and then uses
/// this mask. Each row has whole 16-bit words, with the left pixel in the
/// high bit.
fn and_mask(image: &CursorImage) -> Vec<u8> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let row_bytes = width.div_ceil(16) * 2;
    let mut mask = vec![0xffu8; row_bytes * height];
    for (i, pixel) in image.bgra().chunks_exact(4).enumerate() {
        let (y, x) = (i / width, i % width);
        if pixel[3] != 0 {
            mask[y * row_bytes + x / 8] &= !(0x80 >> (x % 8));
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::and_mask;
    use gpui::CursorImage;

    #[test]
    fn the_mask_shows_the_screen_where_the_image_is_clear() {
        let (width, height) = (18, 3);
        let mut bgra = vec![0; width * height * 4];
        // Opaque pixels at (0, 0), (9, 0) and (17, 1).
        for i in [0, 9, width + 17] {
            bgra[i * 4 + 3] = 255;
        }
        let image = CursorImage::new(width as u32, height as u32, (0, 0), 1.0, bgra).unwrap();
        let mask = and_mask(&image);
        // 18 pixels take two 16-bit words per row.
        assert_eq!(mask.len(), 4 * height);
        assert_eq!(&mask[..8], [0x7f, 0xbf, 0xff, 0xff, 0xff, 0xff, 0xbf, 0xff]);
        assert!(mask[8..].iter().all(|&b| b == 0xff));
    }
}
