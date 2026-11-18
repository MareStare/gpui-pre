use gpui::GpuFailure;

/// The renderer drops its GPU caches at each multiple of this number of
/// consecutive failed frames.
const RECOVERY_INTERVAL: u32 = 6;

/// After this number of consecutive failed frames, the failure is persistent.
const PERSISTENT_AFTER: u32 = 10;

/// The consecutive frames of a renderer that failed with a GPU error.
#[derive(Debug, Default)]
pub(crate) struct FrameFailures {
    failure: Option<Failure>,

    /// The renderer submitted GPU work after the last check. Only then does
    /// a check with no error tell that the work succeeded.
    submitted: bool,
}

#[derive(Debug)]
struct Failure {
    /// The number of consecutive failed frames.
    count: u32,
    last_error: String,
}

/// What the renderer does with a frame after a check.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FrameCheck {
    Draw,

    /// Drop the GPU caches, draw nothing and ask for a new frame.
    Recover,
}

impl FrameFailures {
    /// Records the error that the GPU raised since the last check, if any.
    pub(crate) fn check(&mut self, error: Option<String>) -> FrameCheck {
        let submitted = std::mem::take(&mut self.submitted);
        let Some(error) = error else {
            if submitted {
                self.failure = None;
            }
            return FrameCheck::Draw;
        };
        let count = self.failure.as_ref().map_or(0, |failure| failure.count) + 1;
        log::error!("GPU error in frame {count} of a failure: {error}");
        self.failure = Some(Failure {
            count,
            last_error: error,
        });
        if count.is_multiple_of(RECOVERY_INTERVAL) {
            FrameCheck::Recover
        } else {
            FrameCheck::Draw
        }
    }

    /// Records that the renderer submitted GPU work for a frame.
    pub(crate) fn submit(&mut self) {
        self.submitted = true;
    }

    pub(crate) fn failure(&self) -> Option<GpuFailure> {
        let failure = self.failure.as_ref()?;
        (failure.count > PERSISTENT_AFTER).then(|| GpuFailure {
            error: failure.last_error.clone(),
            failed_frames: failure.count,
        })
    }

    #[cfg(test)]
    fn count(&self) -> u32 {
        self.failure.as_ref().map_or(0, |failure| failure.count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks a frame that raises `error` and submits its work when it draws.
    fn frame(failures: &mut FrameFailures, error: Option<&str>) -> FrameCheck {
        let check = failures.check(error.map(str::to_owned));
        if check == FrameCheck::Draw {
            failures.submit();
        }
        check
    }

    #[test]
    fn a_persistent_error_reaches_the_app_and_the_renderer_keeps_recovering() {
        let mut failures = FrameFailures::default();
        let mut recoveries = Vec::new();
        for index in 1..=PERSISTENT_AFTER {
            if frame(&mut failures, Some("bad bind group")) == FrameCheck::Recover {
                recoveries.push(index);
            }
            assert_eq!(failures.failure(), None);
        }

        frame(&mut failures, Some("bad bind group"));
        let failure = GpuFailure {
            error: "bad bind group".to_owned(),
            failed_frames: PERSISTENT_AFTER + 1,
        };
        assert_eq!(failures.failure(), Some(failure));

        assert_eq!(
            frame(&mut failures, Some("bad bind group")),
            FrameCheck::Recover
        );
        assert_eq!(recoveries, [RECOVERY_INTERVAL]);
        assert_eq!(
            failures.failure().unwrap().failed_frames,
            2 * RECOVERY_INTERVAL
        );
    }

    #[test]
    fn a_frame_with_no_error_ends_the_failure() {
        let mut failures = FrameFailures::default();
        for _ in 0..=PERSISTENT_AFTER {
            frame(&mut failures, Some("bad bind group"));
        }
        assert!(failures.failure().is_some());

        assert_eq!(frame(&mut failures, None), FrameCheck::Draw);
        assert_eq!(failures.failure(), None);
        frame(&mut failures, Some("bad bind group"));
        assert_eq!(failures.count(), 1);
    }

    #[test]
    fn a_transient_error_recovers_with_no_failure() {
        let mut failures = FrameFailures::default();
        for _ in 0..RECOVERY_INTERVAL - 1 {
            assert_eq!(frame(&mut failures, Some("lost texture")), FrameCheck::Draw);
        }
        assert_eq!(
            frame(&mut failures, Some("lost texture")),
            FrameCheck::Recover
        );
        assert_eq!(frame(&mut failures, None), FrameCheck::Draw);
        assert_eq!(frame(&mut failures, None), FrameCheck::Draw);
        assert_eq!(failures.failure(), None);
        assert_eq!(failures.count(), 0);
    }

    /// A recovery submits no work, so the next frame with no error does not end
    /// the failure. The frame after it, which drew, does.
    #[test]
    fn a_frame_after_a_recovery_keeps_the_count() {
        let mut failures = FrameFailures::default();
        for _ in 0..RECOVERY_INTERVAL {
            frame(&mut failures, Some("bad bind group"));
        }
        frame(&mut failures, None);
        assert_eq!(failures.count(), RECOVERY_INTERVAL);
        frame(&mut failures, None);
        assert_eq!(failures.count(), 0);
    }
}
