# gpui-pre with patches

The [gpui-pre](https://crates.io/crates/gpui-pre) crates are a snapshot of
the GPUI crates of [Zed](https://github.com/zed-industries/zed), published
on crates.io. This repo holds the crates of that snapshot that the patches
change, under `crates/`. Use them in place of the crates.io versions:

```toml
[patch.crates-io]
gpui-pre         = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-apple   = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-macos   = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-wgpu    = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-windows = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
```

## Build

The root workspace builds the crates together, with each one patched to
its copy here: `cargo build -p gpui-pre-wgpu`.

The unit tests of a crate need its `test-support` feature:
`cargo test -p gpui-pre-wgpu --features test-support`. The tests of
`gpui-pre-wgpu` read fonts of Zed from `assets/fonts`.

## Layout of the history

1. One commit adds the published sources of one gpui-pre version, with no
   changes.
2. Each patch is one commit on top of it, so each one rebases on its own
   onto a later gpui-pre version.

To move to a later version, add its published sources in a new base
commit, then rebase the patch commits onto it.

## Patches

- Transformed image sprites: an image sprite can have a transform and a
  source rectangle of its texture, and can sample it with a filter that
  keeps thin lines when it is drawn smaller. The filter reads no texels
  of the images next to the sprite in the atlas.
- wgpu by default on Windows and macOS: `GPUI_RENDERER=dx11` or
  `GPUI_RENDERER=metal` picks the native renderer. A window whose wgpu
  renderer fails to start uses the native renderer. On macOS the wgpu
  renderer draws to the `CAMetalLayer` of the view, in Display P3 when the
  window asks for it, and transparent windows show what is behind them.
  An unknown `GPUI_RENDERER` logs a warning and picks wgpu.
  `GPUI_PRESENT_MODE` picks the present mode of the wgpu surface.
- Backends from the environment: `WGPU_BACKEND` and the wgpu backend
  options override the default, DX12 and Vulkan on Windows.
- Custom surfaces: `Window::paint_custom_surface` paints a surface that
  code outside the renderer draws with wgpu. `gpui-pre-wgpu` exports
  `WgpuSurface`, `WgpuSurfaceFrame` and `wgpu_custom_surface`.
- Device loss recovery: the wgpu window renderer of macOS and Windows makes
  a new device after a lost one, with a new identity, and asks the next
  frame to build the scene again.
- Startup speed: the render pipelines build in parallel, wgpu does not
  validate indirect calls, DX12 starts only when no Vulkan adapter can
  draw to the window, Apple tries the integrated GPU first, and a resize
  does not wait for the GPU.
- Windows text: the wgpu renderer takes the DirectWrite text contrast and
  ClearType glyphs as BGRA. `ZED_DEVICE_ID` picks the Direct3D 11 adapter.
- Cached views: a cached view keeps its window control areas, such as
  the drag area of a title bar, in the frames that reuse its paint. Tests
  run the hit test of the window control areas with
  `VisualTestContext::hit_test_window_control_area`.
- Logs: `GPUI_FRAME_LOG` logs the stages of each frame, and
  `GPUI_STARTUP_LOG` the startup stages of the renderers.

## License

Apache-2.0, as the crates it holds. See `LICENSE-APACHE`.
