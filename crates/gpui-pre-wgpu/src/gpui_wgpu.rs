mod cosmic_text_system;
mod frame_failures;
mod frame_log;
mod renderer_choice;
mod startup_log;
mod wgpu_atlas;
mod wgpu_context;
mod wgpu_custom_surface;
mod wgpu_renderer;
#[cfg(not(target_family = "wasm"))]
mod window_renderer;

pub use cosmic_text_system::*;
pub use frame_log::time_frame_draw;
pub use renderer_choice::wgpu_requested;
pub use startup_log::{StartupStage, time_startup_stage};
pub use wgpu;
pub use wgpu_atlas::*;
pub use wgpu_context::*;
pub use wgpu_custom_surface::{WgpuSurface, WgpuSurfaceFrame, wgpu_custom_surface};
#[cfg(all(
    not(target_family = "wasm"),
    any(test, feature = "bench-support", feature = "test-support")
))]
pub use wgpu_renderer::WgpuHeadlessRenderer;
pub use wgpu_renderer::{GpuContext, TextContrast, WgpuRenderer, WgpuSurfaceConfig};
#[cfg(not(target_family = "wasm"))]
pub use window_renderer::WgpuWindowRenderer;
