//! Renders to a texture in place of a surface and reads the pixels back on any thread.

use super::WgpuRendererCore;
use crate::DeviceErrorState;
use anyhow::{Context as _, Result};
use gpui::{DevicePixels, Scene, Size};
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};

/// A texture that a frame renders to in place of a surface, and that copies back to the CPU.
pub(super) struct OffscreenTarget {
    pub(super) texture: wgpu::Texture,
    view: wgpu::TextureView,

    /// A readback buffer that no image uses. The first copy to a new buffer costs the CPU
    /// some milliseconds on some drivers, as the driver commits its memory.
    pub(super) spare_buffer: Arc<Mutex<Option<wgpu::Buffer>>>,
}

impl OffscreenTarget {
    pub(super) fn new(core: &WgpuRendererCore, size: Size<DevicePixels>) -> Result<Self> {
        core.check_target_size(size)?;
        let texture = core
            .resources
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("offscreen_render_target"),
                size: wgpu::Extent3d {
                    width: size.width.0 as u32,
                    height: size.height.0 as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: core.target_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            texture,
            view,
            spare_buffer: Arc::default(),
        })
    }

    pub(super) fn size(&self) -> Size<DevicePixels> {
        Size {
            width: DevicePixels(self.texture.width() as i32),
            height: DevicePixels(self.texture.height() as i32),
        }
    }

    /// Renders the scene to the target. The arguments after `scene` are as for
    /// `WgpuRendererCore::render_frame`.
    #[cfg(any(test, feature = "headless"))]
    pub(super) fn render(
        &self,
        core: &mut WgpuRendererCore,
        scene: &Scene,
        premultiplied_alpha: bool,
        clear_color: wgpu::Color,
    ) -> Result<()> {
        core.render_frame(
            scene,
            &self.view,
            self.size(),
            premultiplied_alpha,
            clear_color,
            |_| {},
        )?;
        Ok(())
    }

    /// Renders the scene to the target and copies the target to a buffer that the CPU
    /// reads, in one submission: on some drivers a second submission waits until the GPU
    /// is done with the first. The image waits for the GPU and fails on a GPU error of the
    /// device after this call.
    pub(super) fn render_to_image(
        &self,
        core: &mut WgpuRendererCore,
        scene: &Scene,
        premultiplied_alpha: bool,
        clear_color: wgpu::Color,
    ) -> Result<gpui::PendingImage> {
        let readback = Readback::new(core, self)?;
        core.render_frame(
            scene,
            &self.view,
            self.size(),
            premultiplied_alpha,
            clear_color,
            |encoder| readback.record_copy(encoder),
        )?;
        Ok(readback.finish())
    }
}

/// A copy of a texture in a buffer that the CPU maps once the GPU is done.
struct Readback {
    device: Arc<wgpu::Device>,
    texture: wgpu::Texture,
    buffer: wgpu::Buffer,

    /// Gets `buffer` back after the read, for the next copy of `texture`.
    spare_buffer: Arc<Mutex<Option<wgpu::Buffer>>>,
    bytes_per_row: u32,

    /// The size of a row in `buffer`, which wgpu aligns.
    padded_bytes_per_row: u32,
    swap_red_blue: bool,
    errors: Arc<DeviceErrorState>,

    /// The generation of `errors` before the render.
    error_generation: u64,
}

impl Readback {
    /// The time between two polls of the device.
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);

    /// Takes a buffer for a copy of all of `target`.
    fn new(core: &WgpuRendererCore, target: &OffscreenTarget) -> Result<Self> {
        let device = &core.resources.device;
        let texture = &target.texture;
        let swap_red_blue = match texture.format() {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => false,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => true,
            format => anyhow::bail!("Cannot read back a render target of format {format:?}"),
        };
        let bytes_per_row = texture
            .width()
            .checked_mul(4)
            .context("Render target row size overflowed")?;
        let padded_bytes_per_row = bytes_per_row
            .checked_next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            .context("Padded row size overflowed")?;
        let buffer_size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(texture.height()))
            .context("Readback buffer size overflowed")?;
        anyhow::ensure!(
            buffer_size <= device.limits().max_buffer_size,
            "Readback buffer size {buffer_size} exceeds maximum buffer size {}",
            device.limits().max_buffer_size
        );
        let spare_buffer = target.spare_buffer.lock().take();
        let buffer = spare_buffer.unwrap_or_else(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("offscreen_readback_buffer"),
                size: buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        Ok(Self {
            device: Arc::clone(device),
            texture: texture.clone(),
            buffer,
            spare_buffer: Arc::clone(&target.spare_buffer),
            bytes_per_row,
            padded_bytes_per_row,
            swap_red_blue,
            errors: Arc::clone(&core.errors),
            error_generation: core.errors.current_generation(),
        })
    }

    fn record_copy(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.texture.height()),
                },
            },
            self.texture.size(),
        );
    }

    /// Maps the buffer after the submission of the copy.
    fn finish(self) -> gpui::PendingImage {
        let (sender, mapped) = std::sync::mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                // The image can be dropped before the GPU is done.
                sender.send(result).ok();
            });
        gpui::PendingImage::new(move || self.read(&mapped))
    }

    /// Waits for the mapping and copies the pixels to an image.
    fn read(
        self,
        mapped: &Receiver<Result<(), wgpu::BufferAsyncError>>,
    ) -> Result<image::RgbaImage> {
        let Self {
            device,
            texture,
            buffer,
            spare_buffer,
            bytes_per_row,
            padded_bytes_per_row,
            swap_red_blue,
            errors,
            mut error_generation,
        } = self;
        Self::wait_for_mapping(&device, mapped)?;
        let (width, height) = (texture.width(), texture.height());
        let mapped_data = buffer.slice(..).get_mapped_range()?;
        let pixel_capacity = usize::try_from(u64::from(bytes_per_row) * u64::from(height))
            .map_err(|_| anyhow::anyhow!("Image size exceeds addressable memory"))?;
        let mut pixels = Vec::with_capacity(pixel_capacity);
        for row in mapped_data
            .chunks_exact(padded_bytes_per_row as usize)
            .take(height as usize)
        {
            pixels.extend_from_slice(&row[..bytes_per_row as usize]);
        }
        drop(mapped_data);
        buffer.unmap();
        *spare_buffer.lock() = Some(buffer);
        if let Some(error) = errors.observe_error(&mut error_generation) {
            anyhow::bail!("GPU error while rendering to an image: {error}");
        }

        if swap_red_blue {
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
        }

        image::RgbaImage::from_raw(width, height, pixels)
            .context("Failed to create an image from the pixel data")
    }

    /// Polls the device until `receiver` gets the result of the mapping.
    ///
    /// A poll that waits holds a lock of the device until the GPU is done, and a frame that
    /// presents on another thread waits for that lock. A poll that does not wait releases it
    /// at once. A poll or a submission on another thread can also deliver the result.
    fn wait_for_mapping(
        device: &wgpu::Device,
        receiver: &Receiver<Result<(), wgpu::BufferAsyncError>>,
    ) -> Result<()> {
        let deadline = std::time::Instant::now() + gpui::PendingImage::TIMEOUT;
        loop {
            device
                .poll(wgpu::PollType::Poll)
                .map_err(|error| anyhow::anyhow!("Failed to poll the GPU device: {error}"))?;
            match receiver.recv_timeout(Self::POLL_INTERVAL) {
                Ok(result) => {
                    return result.map_err(|error| {
                        anyhow::anyhow!("Failed to map the readback buffer: {error}")
                    });
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("The GPU dropped the mapping of the readback buffer");
                }
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "The GPU did not finish the render in {:?}",
                gpui::PendingImage::TIMEOUT
            );
        }
    }
}
