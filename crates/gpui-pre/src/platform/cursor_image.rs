//! Cursors made from images.

use crate::CursorStyle;
use anyhow::{Result, ensure};
use collections::FxHashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

/// The cursor that a window shows: an image, or a style. A platform with no
/// image cursors, or one that cannot take the image, shows the style.
#[derive(Clone, Debug, Default)]
pub struct CursorRequest {
    /// The style that shows where the image cannot.
    pub style: CursorStyle,

    /// The image of the cursor.
    pub image: Option<Arc<CursorImage>>,
}

impl From<CursorStyle> for CursorRequest {
    fn from(style: CursorStyle) -> Self {
        Self { style, image: None }
    }
}

/// Two requests are equal when they have the same style and the same image.
impl PartialEq for CursorRequest {
    fn eq(&self, other: &Self) -> bool {
        let image_id = |request: &Self| request.image.as_ref().map(|image| image.id());
        self.style == other.style && image_id(self) == image_id(other)
    }
}

impl Eq for CursorRequest {}

/// The native cursors of cursor images, by image. Each insert drops the
/// native cursors of the images that dropped. A platform keeps the image of
/// the cursor that it shows, so that the native cursor stays.
pub struct CursorImageCache<T> {
    entries: FxHashMap<CursorImageId, (Weak<CursorImage>, T)>,
}

impl<T> Default for CursorImageCache<T> {
    fn default() -> Self {
        Self {
            entries: FxHashMap::default(),
        }
    }
}

impl<T> CursorImageCache<T> {
    /// The native cursor of `image`. `create` makes it on the first call
    /// for the image.
    pub fn get_or_insert_with(
        &mut self,
        image: &Arc<CursorImage>,
        create: impl FnOnce(&CursorImage) -> T,
    ) -> &T {
        let id = image.id();
        if !self.entries.contains_key(&id) {
            self.entries
                .retain(|_, (image, _)| image.strong_count() > 0);
            self.entries
                .insert(id, (Arc::downgrade(image), create(image)));
        }
        &self.entries[&id].1
    }
}

/// The identity of a [`CursorImage`].
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
    use super::{CursorImage, CursorImageCache, CursorRequest};
    use crate::CursorStyle;
    use std::rc::Rc;
    use std::sync::Arc;

    fn image() -> Arc<CursorImage> {
        Arc::new(CursorImage::new(1, 1, (0, 0), 1.0, vec![0; 4]).unwrap())
    }

    #[test]
    fn requests_with_the_same_style_and_image_are_equal() {
        let (a, b) = (image(), image());
        let request = |style, image: &Arc<CursorImage>| CursorRequest {
            style,
            image: Some(image.clone()),
        };
        assert_eq!(
            request(CursorStyle::Arrow, &a),
            request(CursorStyle::Arrow, &a)
        );
        assert_ne!(
            request(CursorStyle::Arrow, &a),
            request(CursorStyle::Arrow, &b)
        );
        assert_ne!(
            request(CursorStyle::Arrow, &a),
            request(CursorStyle::IBeam, &a)
        );
        assert_ne!(request(CursorStyle::Arrow, &a), CursorStyle::Arrow.into());
        assert_eq!(CursorRequest::default(), CursorStyle::Arrow.into());
    }

    #[test]
    fn a_cache_drops_the_native_cursors_of_dropped_images() {
        let native = Rc::new(());
        let mut cache = CursorImageCache::default();
        let (a, b) = (image(), image());
        cache.get_or_insert_with(&a, |_| native.clone());
        cache.get_or_insert_with(&a, |_| unreachable!("a cursor is made once"));
        assert_eq!(Rc::strong_count(&native), 2);
        drop(a);
        assert_eq!(Rc::strong_count(&native), 2, "a drop waits for an insert");
        cache.get_or_insert_with(&b, |_| native.clone());
        assert_eq!(Rc::strong_count(&native), 2, "the cursor of a dropped");
    }

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
