//! Cursors made from images.

use anyhow::{Result, ensure};
use std::sync::atomic::{AtomicU64, Ordering};

/// The identity of a [`CursorImage`]. A platform keeps the native cursor of
/// each image under its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CursorImageId(u64);

/// A cursor image of BGRA pixels with straight alpha, row by row from the
/// top. The size is in physical pixels at `scale`.
#[derive(Debug)]
pub struct CursorImage {
    id: CursorImageId,
    width: u32,
    height: u32,
    hotspot: (u32, u32),
    scale: f32,
    bgra: Vec<u8>,
}

impl CursorImage {
    /// An image of `width` by `height` pixels for a display of `scale`.
    /// `hotspot` is the pixel that points.
    pub fn new(
        width: u32,
        height: u32,
        hotspot: (u32, u32),
        scale: f32,
        bgra: Vec<u8>,
    ) -> Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        ensure!(width > 0 && height > 0, "a cursor image has no pixels");
        ensure!(
            bgra.len() as u64 == u64::from(width) * u64::from(height) * 4,
            "{} bytes for a cursor image of {width}x{height}",
            bgra.len()
        );
        ensure!(
            hotspot.0 < width && hotspot.1 < height,
            "the hotspot {hotspot:?} is outside of {width}x{height}"
        );
        ensure!(scale > 0.0, "a cursor image of scale {scale}");
        Ok(Self {
            id: CursorImageId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            width,
            height,
            hotspot,
            scale,
            bgra,
        })
    }

    /// The identity of the image.
    pub fn id(&self) -> CursorImageId {
        self.id
    }

    /// The width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel that points.
    pub fn hotspot(&self) -> (u32, u32) {
        self.hotspot
    }

    /// The scale of the display that the image is for.
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The pixels with straight alpha.
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }

    /// The pixels with the color multiplied by the alpha.
    pub fn premultiplied_bgra(&self) -> Vec<u8> {
        let mut pixels = self.bgra.clone();
        for pixel in pixels.chunks_exact_mut(4) {
            let alpha = u16::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
        pixels
    }
}

#[cfg(test)]
mod tests {
    use super::CursorImage;

    #[test]
    fn an_image_checks_its_pixels_and_hotspot() {
        assert!(CursorImage::new(2, 2, (0, 0), 1.0, vec![0; 15]).is_err());
        assert!(CursorImage::new(2, 2, (2, 0), 1.0, vec![0; 16]).is_err());
        let a = CursorImage::new(2, 2, (1, 1), 1.0, vec![0; 16]).unwrap();
        let b = CursorImage::new(2, 2, (1, 1), 1.0, vec![0; 16]).unwrap();
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn premultiplied_pixels_scale_the_color_by_the_alpha() {
        let image =
            CursorImage::new(2, 1, (0, 0), 1.0, vec![255, 128, 0, 128, 9, 9, 9, 0]).unwrap();
        assert_eq!(image.premultiplied_bgra(), [128, 64, 0, 128, 0, 0, 0, 0]);
    }
}
