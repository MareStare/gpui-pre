use anyhow::Result;
use image::RgbaImage;

/// An image of a scene that the GPU may still be drawing.
///
/// The renderer records and submits the draw on the thread that makes the
/// value. [`PendingImage::wait`] waits for the GPU and reads the pixels back.
/// It blocks, so call it off the UI thread.
#[must_use = "the image is lost unless `wait` reads it"]
pub struct PendingImage(Box<dyn FnOnce() -> Result<RgbaImage> + Send>);

impl PendingImage {
    /// The longest time that a renderer waits for the GPU to draw an image.
    pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// An image that `finish` reads once the GPU is done.
    pub fn new(finish: impl FnOnce() -> Result<RgbaImage> + Send + 'static) -> Self {
        Self(Box::new(finish))
    }

    /// An image that the renderer has already read.
    pub fn ready(image: RgbaImage) -> Self {
        Self::new(move || Ok(image))
    }

    /// Waits for the GPU and returns the image.
    pub fn wait(self) -> Result<RgbaImage> {
        (self.0)()
    }
}

impl std::fmt::Debug for PendingImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingImage").finish_non_exhaustive()
    }
}
