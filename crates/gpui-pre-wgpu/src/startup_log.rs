//! `GPUI_STARTUP_LOG=1` prints the time of each startup stage of the renderers to stderr.
//! A line is `proto startup-<stage> <ms> <end>`. `<end>` is the Unix time in ms at the end
//! of the stage, so a launcher can put the stages on the timeline of the process.
//! A `window-renderer` line has a fifth field: the name of the renderer that started.

use std::fmt;
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn startup_log_enabled() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var_os("GPUI_STARTUP_LOG").is_some());
    *ON
}

/// The renderer that a window renderer start gave.
pub enum StartedRenderer {
    /// A wgpu renderer with the name of its backend, e.g. `vulkan`. Its log name is
    /// that name.
    Wgpu(&'static str),
    /// A native renderer of the platform, e.g. `dx11`. Its log name is `native:<name>`.
    Native(&'static str),
}

impl fmt::Display for StartedRenderer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wgpu(backend) => f.write_str(backend),
            Self::Native(name) => write!(f, "native:{name}"),
        }
    }
}

/// Runs `work` and logs its time as `stage`. It reads no clock while the log is off.
pub fn time_startup_stage<T>(stage: &str, work: impl FnOnce() -> T) -> T {
    let (result, time) = timed(work);
    if let Some(time) = time {
        eprintln!("{}", time.line(stage));
    }
    result
}

/// Runs `start`, the start of the renderer of a window, and logs its time. The stage
/// holds the other stages of that start.
///
/// A start that gives a renderer logs the stage `window-renderer` with the renderer
/// that `name_of` gives. A start that fails logs the stage `window-renderer-failed`.
pub fn time_window_renderer<R>(
    start: impl FnOnce() -> anyhow::Result<R>,
    name_of: impl FnOnce(&R) -> StartedRenderer,
) -> anyhow::Result<R> {
    let (started, time) = timed(start);
    if let Some(time) = time {
        eprintln!("{}", window_renderer_line(&started, name_of, &time));
    }
    started
}

/// The time of a stage.
struct StageTime {
    ms: f64,
    /// The Unix time in ms at the end of the stage.
    end: f64,
}

impl StageTime {
    fn line(&self, stage: &str) -> String {
        let Self { ms, end } = self;
        format!("proto startup-{stage} {ms:.3} {end:.1}")
    }
}

/// Runs `work` and gives its time while the log is on.
fn timed<T>(work: impl FnOnce() -> T) -> (T, Option<StageTime>) {
    if !startup_log_enabled() {
        return (work(), None);
    }
    let start = Instant::now();
    let result = work();
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let end = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64() * 1000.0);
    (result, Some(StageTime { ms, end }))
}

fn window_renderer_line<R>(
    started: &anyhow::Result<R>,
    name_of: impl FnOnce(&R) -> StartedRenderer,
    time: &StageTime,
) -> String {
    match started {
        Ok(started) => format!("{} {}", time.line("window-renderer"), name_of(started)),
        Err(_) => time.line("window-renderer-failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::{StageTime, StartedRenderer, window_renderer_line};

    const TIME: StageTime = StageTime { ms: 1.5, end: 2.0 };

    #[test]
    fn a_window_renderer_logs_the_renderer_that_started() {
        let line = window_renderer_line(&Ok(()), |()| StartedRenderer::Wgpu("vulkan"), &TIME);
        assert_eq!(line, "proto startup-window-renderer 1.500 2.0 vulkan");
        let line = window_renderer_line(&Ok(()), |()| StartedRenderer::Native("dx11"), &TIME);
        assert_eq!(line, "proto startup-window-renderer 1.500 2.0 native:dx11");
    }

    #[test]
    fn a_failed_window_renderer_logs_its_own_stage() {
        let started: anyhow::Result<()> = Err(anyhow::anyhow!("no adapter"));
        let line = window_renderer_line(&started, |()| StartedRenderer::Wgpu("vulkan"), &TIME);
        assert_eq!(line, "proto startup-window-renderer-failed 1.500 2.0");
    }
}
