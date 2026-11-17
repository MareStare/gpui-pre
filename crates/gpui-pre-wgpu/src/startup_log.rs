//! `GPUI_STARTUP_LOG=1` prints the time of each startup stage of the renderers to stderr.
//! A line is `proto startup-<stage> <ms> <end>`. `<end>` is the Unix time in ms at the end
//! of the stage, so a launcher can put the stages on the timeline of the process.
//! A `window-renderer` line has a fifth field: the name of the renderer that started.

use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn startup_log_enabled() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("GPUI_STARTUP_LOG").is_some());
    *ON
}

/// Runs `work` and logs its time as `stage`. It reads no clock while the log is off.
pub fn time_startup_stage<T>(stage: &str, work: impl FnOnce() -> T) -> T {
    let (result, time) = timed(work);
    if let Some((ms, end)) = time {
        eprintln!("{}", stage_line(stage, ms, end, None));
    }
    result
}

/// Runs `start`, the start of the renderer of a window, and logs its time. The stage
/// holds the other stages of that start.
///
/// A start that gives a renderer logs the stage `window-renderer`, and after `<end>` the
/// name that `renderer` gives: the wgpu backend, e.g. `vulkan`, or `native:<name>` for a
/// native renderer. A start that fails logs the stage `window-renderer-failed`.
pub fn time_window_renderer<R>(
    start: impl FnOnce() -> anyhow::Result<R>,
    renderer: impl FnOnce(&R) -> &'static str,
) -> anyhow::Result<R> {
    let (started, time) = timed(start);
    if let Some((ms, end)) = time {
        let (stage, name) = window_renderer_stage(&started, renderer);
        eprintln!("{}", stage_line(stage, ms, end, name));
    }
    started
}

/// Runs `work` and gives its time in ms and the Unix time in ms at its end, while the
/// log is on.
fn timed<T>(work: impl FnOnce() -> T) -> (T, Option<(f64, f64)>) {
    if !startup_log_enabled() {
        return (work(), None);
    }
    let start = Instant::now();
    let result = work();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let end = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0);
    (result, Some((ms, end)))
}

fn window_renderer_stage<R>(
    started: &anyhow::Result<R>,
    renderer: impl FnOnce(&R) -> &'static str,
) -> (&'static str, Option<&'static str>) {
    match started {
        Ok(started) => ("window-renderer", Some(renderer(started))),
        Err(_) => ("window-renderer-failed", None),
    }
}

fn stage_line(stage: &str, ms: f64, end: f64, renderer: Option<&str>) -> String {
    match renderer {
        Some(renderer) => format!("proto startup-{stage} {ms:.3} {end:.1} {renderer}"),
        None => format!("proto startup-{stage} {ms:.3} {end:.1}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{stage_line, window_renderer_stage};

    #[test]
    fn a_window_renderer_logs_the_renderer_that_started() {
        let started: anyhow::Result<&str> = Ok("native:dx11");
        let (stage, renderer) = window_renderer_stage(&started, |name| name);
        assert_eq!(
            stage_line(stage, 1.5, 2.0, renderer),
            "proto startup-window-renderer 1.500 2.0 native:dx11"
        );
    }

    #[test]
    fn a_failed_window_renderer_logs_its_own_stage() {
        let started: anyhow::Result<&str> = Err(anyhow::anyhow!("no adapter"));
        let (stage, renderer) = window_renderer_stage(&started, |name| name);
        assert_eq!(
            stage_line(stage, 1.5, 2.0, renderer),
            "proto startup-window-renderer-failed 1.500 2.0"
        );
    }
}
