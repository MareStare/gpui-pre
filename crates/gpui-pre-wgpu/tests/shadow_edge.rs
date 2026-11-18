//! A shadow anti-aliases the edge of its shape where the edge lies inside a
//! device pixel, also where the center of that pixel lies outside the shape.

use gpui::{
    Bounds, ContentMask, Corners, DevicePixels, PlatformHeadlessRenderer, Point, ScaledPixels,
    Scene, Shadow, Size, hsla,
};
use gpui_wgpu::WgpuHeadlessRenderer;

#[test]
fn a_shadow_covers_part_of_the_pixels_at_its_fractional_edges() -> anyhow::Result<()> {
    let mut renderer = match WgpuHeadlessRenderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("The host has no headless renderer ({error}). The test checks nothing.");
            return Ok(());
        }
    };
    let (width, height) = (16, 10);
    let device = |x: f32, y: f32, width: f32, height: f32| Bounds {
        origin: Point {
            x: ScaledPixels(x),
            y: ScaledPixels(y),
        },
        size: Size {
            width: ScaledPixels(width),
            height: ScaledPixels(height),
        },
    };
    // The left edge covers a quarter of column 4, the right edge a quarter
    // of column 12.
    let shape = device(4.75, 2.75, 7.5, 4.5);
    let mut scene = Scene::default();
    scene.insert_primitive(Shadow {
        order: 0,
        blur_radius: ScaledPixels(0.),
        bounds: shape,
        corner_radii: Corners::default(),
        content_mask: ContentMask {
            bounds: device(0., 0., width as f32, height as f32),
        },
        color: hsla(0., 0., 1., 1.),
        element_bounds: shape,
        element_corner_radii: Corners::default(),
        inset: 0,
        pad: 0,
    });
    scene.finish();

    let image = renderer.render_scene_to_image(
        &scene,
        Size {
            width: DevicePixels(width),
            height: DevicePixels(height),
        },
    )?;
    let red = |x: u32| image.get_pixel(x, 5).0[0];
    let (empty, full) = (red(0), red(8));
    assert!(
        empty < full,
        "row 5: {:?}",
        (0..16).map(red).collect::<Vec<_>>()
    );
    for x in [3, 13] {
        assert_eq!(red(x), empty, "column {x}");
    }
    for x in [4, 12] {
        assert!(
            empty < red(x) && red(x) < full,
            "column {x}: {} is not between {empty} and {full}",
            red(x)
        );
    }
    Ok(())
}
