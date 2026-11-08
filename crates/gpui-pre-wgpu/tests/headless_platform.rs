//! A headless platform runs an app in real time with no OS window.

#![cfg(not(target_family = "wasm"))]

use std::{cell::Cell, rc::Rc, sync::Arc};

use gpui::{
    AppContext as _, Application, Bounds, Context, InteractiveElement as _, IntoElement,
    NoopTextSystem, ParentElement as _, Pixels, PlatformHeadlessRenderer, Render,
    StatefulInteractiveElement as _, Styled as _, Window, WindowOptions, div, headless_platform,
    point, px, size,
};
use gpui_wgpu::WgpuHeadlessRenderer;

struct NamedBox;

impl Render for NamedBox {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .absolute()
                .left(px(10.0))
                .top(px(20.0))
                .size(px(30.0))
                .debug_selector(|| "box".to_owned())
                .child(div().id("press-me").size(px(6.0)).on_click(|_, _, _| {})),
        )
    }
}

#[test]
fn a_headless_app_draws_a_frame_and_runs_its_quit_callbacks() {
    let quit = Rc::new(Cell::new(false));
    let scale = Rc::new(Cell::new(0.0));
    let names = Rc::new(Cell::new(Vec::<(String, Bounds<Pixels>)>::new()));
    let platform = headless_platform(Box::new(|| Ok(None)), Arc::new(NoopTextSystem::new()), 1.5);
    let (on_quit, seen_scale, seen_names) = (quit.clone(), scale.clone(), names.clone());
    let app = Application::with_platform(platform);
    app.on_open_urls(|_| {});
    app.run(move |cx| {
        cx.on_app_quit(move |_| {
            on_quit.set(true);
            async {}
        })
        .detach();
        cx.open_window(WindowOptions::default(), |window, cx| {
            seen_scale.set(window.scale_factor());
            window.on_next_frame(move |window, cx| {
                seen_names.set(window.named_element_bounds().to_vec());
                cx.quit();
            });
            cx.new(|_| NamedBox)
        })
        .unwrap();
    });
    assert!(quit.get());
    assert_eq!(scale.get(), 1.5);
    let expected = Bounds::new(point(px(10.0), px(20.0)), size(px(30.0), px(30.0)));
    let press = Bounds::new(point(px(10.0), px(20.0)), size(px(6.0), px(6.0)));
    let names = names.take();
    assert_eq!(names[0], ("box".to_owned(), expected));
    assert_eq!(names[1], ("press-me".to_owned(), press));
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
