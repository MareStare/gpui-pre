//! An image sprite reads only the texels of its own tile, also where the atlas packs
//! another image next to it.

use std::borrow::Cow;

use gpui::{
    AtlasKey, AtlasTile, Bounds, ContentMask, Corners, DevicePixels, ImageId,
    PlatformHeadlessRenderer, Point, PolychromeSprite, RenderImageParams, Scene, Size,
    TransformationMatrix,
};
use gpui_wgpu::WgpuHeadlessRenderer;

const STRIP_WIDTH: i32 = 8;

fn insert_strip(
    renderer: &WgpuHeadlessRenderer,
    id: usize,
    bgra: [u8; 4],
) -> anyhow::Result<AtlasTile> {
    let key = AtlasKey::Image(RenderImageParams {
        image_id: ImageId(id),
        frame_index: 0,
    });
    let size = Size {
        width: DevicePixels(STRIP_WIDTH),
        height: DevicePixels(1),
    };
    let bytes = bgra.repeat(STRIP_WIDTH as usize);
    renderer
        .sprite_atlas()
        .get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.clone())))))?
        .ok_or_else(|| anyhow::anyhow!("the atlas made no tile"))
}

#[test]
fn a_stretched_image_shows_no_texels_of_its_neighbor() -> anyhow::Result<()> {
    let mut renderer = match WgpuHeadlessRenderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("The host has no headless renderer ({error}). The test checks nothing.");
            return Ok(());
        }
    };
    let red = insert_strip(&renderer, 1, [0, 0, 255, 255])?;
    let blue = insert_strip(&renderer, 2, [255, 0, 0, 255])?;
    assert_eq!(red.texture_id, blue.texture_id);

    // Most rows of the 10 px tall sprite sample near an edge of its 1 px tall tile.
    let (width, height) = (32, 10);
    let bounds = Bounds {
        origin: Point::default(),
        size: Size {
            width: (width as f32).into(),
            height: (height as f32).into(),
        },
    };
    let mut scene = Scene::default();
    scene.insert_primitive(PolychromeSprite {
        order: 0,
        pad: 0,
        grayscale: false.into(),
        opacity: 1.0,
        bounds,
        content_mask: ContentMask { bounds },
        corner_radii: Corners::default(),
        tile: red,
        transformation: TransformationMatrix::unit(),
    });
    scene.finish();

    let image = renderer.render_scene_to_image(
        &scene,
        Size {
            width: DevicePixels(width),
            height: DevicePixels(height),
        },
    )?;
    for (x, y, pixel) in image.enumerate_pixels() {
        assert!(
            pixel
                .0
                .iter()
                .zip([255, 0, 0, 255])
                .all(|(actual, expected)| actual.abs_diff(expected) <= 3),
            "pixel ({x}, {y}) of the red sprite was {:?}",
            pixel.0
        );
    }
    Ok(())
}
