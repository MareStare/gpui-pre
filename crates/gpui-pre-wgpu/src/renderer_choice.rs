//! `GPUI_RENDERER` picks the renderer of the windows. The default is wgpu.

/// Tells whether the windows draw with wgpu. `native` is the value of `GPUI_RENDERER`
/// that picks the native renderer of the platform.
pub fn wgpu_requested(native: &str) -> bool {
    wgpu_requested_by(std::env::var("GPUI_RENDERER").ok().as_deref(), native)
}

fn wgpu_requested_by(value: Option<&str>, native: &str) -> bool {
    match value {
        None | Some("wgpu") => true,
        Some(value) if value == native => false,
        Some(value) => {
            log::warn!(
                "Unknown GPUI_RENDERER {value:?}, using wgpu. The values are wgpu and {native}."
            );
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::wgpu_requested_by;

    #[test]
    fn wgpu_is_the_default() {
        assert!(wgpu_requested_by(None, "metal"));
        assert!(wgpu_requested_by(Some("wgpu"), "metal"));
        assert!(wgpu_requested_by(Some("vulkan"), "metal"));
        assert!(!wgpu_requested_by(Some("metal"), "metal"));
        assert!(!wgpu_requested_by(Some("dx11"), "dx11"));
    }
}
