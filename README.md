# gpui-pre with patches

The [gpui-pre](https://crates.io/crates/gpui-pre) crates are a snapshot of
the GPUI crates of [Zed](https://github.com/zed-industries/zed), published
on crates.io. This repo holds the crates of that snapshot that the patches
change, under `crates/`. Use them in place of the crates.io versions:

```toml
[patch.crates-io]
gpui-pre         = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-apple   = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-wgpu    = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
gpui-pre-windows = { git = "https://github.com/MareStare/gpui-pre", rev = "<commit>" }
```

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
  keeps thin lines when it is drawn smaller.

## License

Apache-2.0, as the crates it holds. See `LICENSE-APACHE`.
