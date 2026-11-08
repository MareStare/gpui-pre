//! A headless platform runs an app in real time with no OS window.

#![cfg(not(target_family = "wasm"))]

use std::{cell::Cell, rc::Rc, sync::Arc};

use gpui::{
    AppContext as _, Application, Empty, NoopTextSystem, PlatformHeadlessRenderer, WindowOptions,
    headless_platform,
};
use gpui_wgpu::WgpuHeadlessRenderer;

#[test]
fn a_headless_app_draws_a_frame_and_runs_its_quit_callbacks() {
    let quit = Rc::new(Cell::new(false));
    let scale = Rc::new(Cell::new(0.0));
    let platform = headless_platform(Box::new(|| Ok(None)), Arc::new(NoopTextSystem::new()), 1.5);
    let (on_quit, seen_scale) = (quit.clone(), scale.clone());
    Application::with_platform(platform).run(move |cx| {
        cx.on_app_quit(move |_| {
            on_quit.set(true);
            async {}
        })
        .detach();
        cx.open_window(WindowOptions::default(), |window, cx| {
            seen_scale.set(window.scale_factor());
            window.on_next_frame(|_, cx| cx.quit());
            cx.new(|_| Empty)
        })
        .unwrap();
    });
    assert!(quit.get());
    assert_eq!(scale.get(), 1.5);
}

#[test]
fn the_wgpu_headless_renderer_draws_custom_surfaces() {
    let renderer = match WgpuHeadlessRenderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("skipped: no GPU adapter: {error:#}");
            return;
        }
    };
    assert!(renderer.draws_custom_surfaces());
}
