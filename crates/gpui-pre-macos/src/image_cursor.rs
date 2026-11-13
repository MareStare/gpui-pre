//! Cursors made from images, as winit makes them (`cursor_from_image`,
//! <https://github.com/rust-windowing/winit/blob/master/winit-appkit/src/cursor.rs>).
//! The bitmap has the pixel size of the image, and the image has its logical
//! size. Thus the cursor is sharp on a display of the scale of the image.
//!
//! The classes: <https://developer.apple.com/documentation/appkit/nscursor> and
//! <https://developer.apple.com/documentation/appkit/nsbitmapimagerep>.

use gpui::{CursorImage, CursorImageCache};
use objc2::{AnyThread as _, rc::Retained};
use objc2_app_kit::{NSBitmapImageRep, NSCursor, NSDeviceRGBColorSpace, NSImage};
use objc2_foundation::{NSPoint, NSSize};
use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::Arc;

thread_local! {
    /// The cursors of the images. None for an image that AppKit did not take.
    static CURSORS: RefCell<CursorImageCache<Option<Retained<NSCursor>>>> =
        RefCell::default();
}

/// The cursor of the image.
pub(crate) fn cursor(image: &Arc<CursorImage>) -> Option<Retained<NSCursor>> {
    CURSORS.with_borrow_mut(|cursors| cursors.get_or_insert_with(image, create).clone())
}

fn create(image: &CursorImage) -> Option<Retained<NSCursor>> {
    let (width, height) = (image.width() as isize, image.height() as isize);
    // SAFETY: a null plane pointer makes the bitmap own its pixels. The
    // sizes describe 8-bit RGBA rows with no padding.
    let bitmap = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            null_mut(),
            width,
            height,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            width * 4,
            32,
        )
    }?;
    // The default format of a bitmap is RGBA with premultiplied alpha.
    let mut pixels = image.premultiplied_bgra();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    // SAFETY: the bitmap owns `width * height * 4` bytes, as many as the
    // image has. Nothing else writes them.
    unsafe {
        let data = bitmap.bitmapData();
        if data.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), data, pixels.len());
    }
    let scale = f64::from(image.scale());
    let logical = NSSize::new(width as f64 / scale, height as f64 / scale);
    bitmap.setSize(logical);
    let ns_image = NSImage::initWithSize(NSImage::alloc(), logical);
    ns_image.addRepresentation(&bitmap);
    let (x, y) = image.hotspot();
    let hotspot = NSPoint::new(f64::from(x) / scale, f64::from(y) / scale);
    Some(NSCursor::initWithImage_hotSpot(
        NSCursor::alloc(),
        &ns_image,
        hotspot,
    ))
}
