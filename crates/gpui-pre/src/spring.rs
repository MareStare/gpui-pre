use std::{ops::RangeInclusive, time::Duration};

use smallvec::{SmallVec, smallvec};

use crate::{Hsla, Pixels, Rems, Rgba};

const CRITICAL_DAMPING_TOLERANCE: f32 = 1e-4;
const DEFAULT_SPRING_EPSILON: f32 = 0.001;

/// The value of $\omega_0 t$ at which a critically damped spring that starts at
/// rest has 1% of its distance left: $(1 + \omega_0 t) e^{-\omega_0 t} = 0.01$.
const CRITICAL_SETTLE_COEFFICIENT: f32 = 6.638_352;

/// The coordinates of a spring target, one spring per coordinate.
pub type SpringCoordinates = SmallVec<[f32; 4]>;

/// The physical parameters of a damped harmonic oscillator.
///
/// `stiffness` and `mass` must be finite and positive. `damping` must be finite
/// and non-negative. As the one exception, an infinite `stiffness` with zero
/// `damping` makes a spring that jumps to its target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringConfig {
    /// The spring stiffness, conventionally written as $k$.
    pub stiffness: f32,
    /// The viscous damping coefficient, conventionally written as $c$.
    pub damping: f32,
    /// The moving mass, conventionally written as $m$.
    pub mass: f32,
}

impl SpringConfig {
    /// Creates a spring from its physical parameters.
    pub const fn new(stiffness: f32, damping: f32, mass: f32) -> Self {
        Self {
            stiffness,
            damping,
            mass,
        }
    }

    /// Creates a critically damped spring with unit mass that comes within 1%
    /// of its target in `duration` when it starts at rest.
    ///
    /// Such a spring never overshoots, and it suits a transition that has a
    /// duration in its design. A zero duration gives a spring that jumps to
    /// its target.
    pub fn critically_damped(duration: Duration) -> Self {
        if duration.is_zero() {
            return Self::new(f32::INFINITY, 0.0, 1.0);
        }
        let natural_frequency = CRITICAL_SETTLE_COEFFICIENT / duration.as_secs_f32();
        Self::new(
            natural_frequency * natural_frequency,
            2.0 * natural_frequency,
            1.0,
        )
    }

    /// Returns the natural angular frequency and damping ratio $(\omega_0, \zeta)$.
    pub fn canonical(&self) -> (f32, f32) {
        let natural_frequency = (self.stiffness / self.mass).sqrt();
        let damping_ratio = self.damping / (2.0 * (self.stiffness * self.mass).sqrt());
        (natural_frequency, damping_ratio)
    }

    /// Advances a spring toward a target that remains fixed for `delta_time`.
    ///
    /// This analytic step is independent of frame rate and preserves velocity,
    /// allowing an interrupted spring to be retargeted without restarting it.
    pub fn step(&self, state: SpringState, target: f32, delta_time: f32) -> SpringState {
        let propagator = self.propagator(delta_time);
        let displacement = state.position - target;

        SpringState {
            position: target + propagator[0][0] * displacement + propagator[0][1] * state.velocity,
            velocity: propagator[1][0] * displacement + propagator[1][1] * state.velocity,
        }
    }

    /// Advances a spring toward a target moving at a constant velocity.
    ///
    /// A first-order hold avoids the frame-rate-dependent lag introduced by
    /// treating a dragged target as stationary between frames.
    pub fn step_ramp(
        &self,
        state: SpringState,
        target: f32,
        target_velocity: f32,
        delta_time: f32,
    ) -> SpringState {
        let (natural_frequency, damping_ratio) = self.canonical();
        let steady_state_lag = -2.0 * damping_ratio * target_velocity / natural_frequency;
        let displacement = state.position - target - steady_state_lag;
        let velocity = state.velocity - target_velocity;
        let propagator = self.propagator(delta_time);
        let target = target + target_velocity * delta_time;

        SpringState {
            position: target
                + steady_state_lag
                + propagator[0][0] * displacement
                + propagator[0][1] * velocity,
            velocity: target_velocity
                + propagator[1][0] * displacement
                + propagator[1][1] * velocity,
        }
    }

    /// Returns the exact state-transition matrix for a constant target.
    ///
    /// Materializing this matrix is useful when many springs share the same
    /// configuration and frame delta. A matrix must not be reused when the
    /// frame delta changes.
    pub fn propagator(&self, delta_time: f32) -> [[f32; 2]; 2] {
        let (natural_frequency, damping_ratio) = self.canonical();
        if natural_frequency.is_infinite() {
            // A spring that jumps is on its target at rest at any time.
            return [[0.0, 0.0], [0.0, 0.0]];
        }

        if damping_ratio < 1.0 - CRITICAL_DAMPING_TOLERANCE {
            let decay = damping_ratio * natural_frequency;
            let damped_frequency = natural_frequency * (1.0 - damping_ratio * damping_ratio).sqrt();
            let exponential = (-decay * delta_time).exp();
            let (sine, cosine) = (damped_frequency * delta_time).sin_cos();
            let sine_over_frequency = sine / damped_frequency;

            [
                [
                    exponential * (cosine + decay * sine_over_frequency),
                    exponential * sine_over_frequency,
                ],
                [
                    -exponential * natural_frequency * natural_frequency * sine_over_frequency,
                    exponential * (cosine - decay * sine_over_frequency),
                ],
            ]
        } else if damping_ratio > 1.0 + CRITICAL_DAMPING_TOLERANCE {
            let root = (damping_ratio * damping_ratio - 1.0).sqrt();
            let root_sum = damping_ratio + root;
            let slow_root = -natural_frequency / root_sum;
            let fast_root = -natural_frequency * root_sum;
            let denominator = slow_root - fast_root;
            let slow_exponential = (slow_root * delta_time).exp();
            let fast_exponential = (fast_root * delta_time).exp();

            [
                [
                    (-fast_root * slow_exponential + slow_root * fast_exponential) / denominator,
                    (slow_exponential - fast_exponential) / denominator,
                ],
                [
                    slow_root * fast_root * (fast_exponential - slow_exponential) / denominator,
                    (slow_root * slow_exponential - fast_root * fast_exponential) / denominator,
                ],
            ]
        } else {
            let exponential = (-natural_frequency * delta_time).exp();

            [
                [
                    exponential * (1.0 + natural_frequency * delta_time),
                    exponential * delta_time,
                ],
                [
                    -exponential * natural_frequency * natural_frequency * delta_time,
                    exponential * (1.0 - natural_frequency * delta_time),
                ],
            ]
        }
    }

    /// Tests both displacement and velocity against a positional tolerance.
    ///
    /// Velocity is compared with `epsilon * natural_frequency`, giving it the
    /// corresponding animated-units-per-second scale. A spring that jumps is
    /// settled when it is on its target, at any velocity.
    pub fn is_settled(&self, state: SpringState, target: f32, epsilon: f32) -> bool {
        let (natural_frequency, _) = self.canonical();
        epsilon.is_finite()
            && epsilon >= 0.0
            && (state.position - target).abs() <= epsilon
            && (natural_frequency.is_infinite()
                || state.velocity.abs() <= epsilon * natural_frequency)
    }

    /// Returns a conservative time after which the spring remains settled.
    ///
    /// An undamped spring has no finite settling time and returns
    /// [`Duration::MAX`].
    pub fn settle_time(&self, state: SpringState, target: f32, epsilon: f32) -> Duration {
        let displacement = state.position - target;
        if displacement == 0.0 && state.velocity == 0.0 {
            return Duration::ZERO;
        }

        let (natural_frequency, damping_ratio) = self.canonical();
        if natural_frequency.is_infinite() {
            return Duration::ZERO;
        }
        if !natural_frequency.is_finite()
            || natural_frequency <= 0.0
            || !damping_ratio.is_finite()
            || damping_ratio <= 0.0
            || !epsilon.is_finite()
            || epsilon <= 0.0
        {
            return Duration::MAX;
        }

        let velocity_threshold = epsilon * natural_frequency;

        if damping_ratio < 1.0 - CRITICAL_DAMPING_TOLERANCE {
            let decay = damping_ratio * natural_frequency;
            let damped_frequency = natural_frequency * (1.0 - damping_ratio * damping_ratio).sqrt();
            let sine_coefficient = (state.velocity + decay * displacement) / damped_frequency;
            let position_envelope = displacement.hypot(sine_coefficient);
            let velocity_cosine = damped_frequency * sine_coefficient - decay * displacement;
            let velocity_sine = -damped_frequency * displacement - decay * sine_coefficient;
            let velocity_envelope = velocity_cosine.hypot(velocity_sine);

            find_settle_time(
                epsilon,
                velocity_threshold,
                0.0,
                natural_frequency,
                move |time| {
                    let exponential = (-decay * time).exp();
                    (
                        position_envelope * exponential,
                        velocity_envelope * exponential,
                    )
                },
            )
        } else if damping_ratio > 1.0 + CRITICAL_DAMPING_TOLERANCE {
            let root = (damping_ratio * damping_ratio - 1.0).sqrt();
            let root_sum = damping_ratio + root;
            let slow_root = -natural_frequency / root_sum;
            let fast_root = -natural_frequency * root_sum;
            let denominator = slow_root - fast_root;
            let slow_coefficient = (state.velocity - fast_root * displacement) / denominator;
            let fast_coefficient = (slow_root * displacement - state.velocity) / denominator;

            find_settle_time(
                epsilon,
                velocity_threshold,
                0.0,
                natural_frequency,
                move |time| {
                    let slow_term = slow_coefficient.abs() * (slow_root * time).exp();
                    let fast_term = fast_coefficient.abs() * (fast_root * time).exp();
                    (
                        slow_term + fast_term,
                        slow_root.abs() * slow_term + fast_root.abs() * fast_term,
                    )
                },
            )
        } else {
            let linear_coefficient = state.velocity + natural_frequency * displacement;
            let position_constant = displacement.abs();
            let position_linear = linear_coefficient.abs();
            let velocity_constant = (linear_coefficient - natural_frequency * displacement).abs();
            let velocity_linear = natural_frequency * linear_coefficient.abs();
            let position_decay_start =
                envelope_decay_start(position_constant, position_linear, natural_frequency);
            let velocity_decay_start =
                envelope_decay_start(velocity_constant, velocity_linear, natural_frequency);

            find_settle_time(
                epsilon,
                velocity_threshold,
                position_decay_start.max(velocity_decay_start),
                natural_frequency,
                move |time| {
                    let exponential = (-natural_frequency * time).exp();
                    (
                        (position_constant + position_linear * time) * exponential,
                        (velocity_constant + velocity_linear * time) * exponential,
                    )
                },
            )
        }
    }
}

/// The instantaneous position and velocity of a spring.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpringState {
    /// The current value in the animated unit.
    pub position: f32,
    /// The current value's change per second.
    pub velocity: f32,
}

/// A value that can be targeted by a one-dimensional spring.
///
/// Implementations may project the spring coordinate into a richer output,
/// allowing a discrete state or a path through a multidimensional value to be
/// driven by one spring.
pub trait SpringTarget: 'static {
    /// The value supplied to the spring animator.
    type Output;

    /// Returns the target in the spring's coordinate space.
    fn target(&self) -> f32;

    /// Projects a spring coordinate into the animated output.
    fn resolve(&self, value: f32) -> Self::Output;
}

/// A value that springs can target, one spring per coordinate.
///
/// Each coordinate gets its own one-dimensional spring with the same
/// configuration, so each coordinate keeps its own velocity. A position or a
/// rectangle has one coordinate per component. Each [`SpringTarget`] is a
/// vector with one coordinate.
pub trait SpringVector: 'static {
    /// The value supplied to the spring animator.
    type Output;

    /// Returns the target in the coordinate space of the springs.
    ///
    /// A target returns the same number of coordinates every time.
    fn coordinates(&self) -> SpringCoordinates;

    /// Projects the coordinates of the springs into the animated output.
    fn resolve_coordinates(&self, coordinates: &[f32]) -> Self::Output;

    /// The settle tolerance of each coordinate, in the unit of that
    /// coordinate, such as half a pixel for a position. With none, each
    /// coordinate takes the epsilon of the animation.
    ///
    /// A target gives one tolerance for each coordinate.
    fn tolerances(&self) -> Option<SpringCoordinates> {
        None
    }
}

impl<T: SpringTarget> SpringVector for T {
    type Output = T::Output;

    fn coordinates(&self) -> SpringCoordinates {
        smallvec![self.target()]
    }

    fn resolve_coordinates(&self, coordinates: &[f32]) -> Self::Output {
        self.resolve(coordinates[0])
    }
}

impl SpringTarget for f32 {
    type Output = f32;

    fn target(&self) -> f32 {
        *self
    }

    fn resolve(&self, value: f32) -> Self::Output {
        value
    }
}

impl SpringTarget for Pixels {
    type Output = Pixels;

    fn target(&self) -> f32 {
        self.as_f32()
    }

    fn resolve(&self, value: f32) -> Self::Output {
        Pixels::from(value)
    }
}

impl SpringTarget for Rems {
    type Output = Rems;

    fn target(&self) -> f32 {
        self.0
    }

    fn resolve(&self, value: f32) -> Self::Output {
        Rems(value)
    }
}

impl SpringTarget for bool {
    type Output = AnimationPhase;

    fn target(&self) -> f32 {
        if *self { 1.0 } else { 0.0 }
    }

    fn resolve(&self, value: f32) -> Self::Output {
        AnimationPhase(value)
    }
}

/// A potentially overshooting coordinate within an animation.
///
/// Phases are not restricted to 0..1. Multi-stage animations can assign each
/// stage its own coordinate and interpolate over ranges such as 1..=2.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct AnimationPhase(
    /// The unbounded phase coordinate.
    pub f32,
);

impl AnimationPhase {
    /// Restricts this phase to the bounds of a range.
    pub fn clamp(self, range: RangeInclusive<f32>) -> Self {
        let (first, second) = range.into_inner();
        Self(self.0.clamp(first.min(second), first.max(second)))
    }

    /// Interpolates between values using 0 and 1 as their phase coordinates.
    pub fn interpolate<T: Interpolate>(self, from: T, to: T) -> T {
        T::interpolate(from, to, self.0)
    }

    /// Interpolates between values without extrapolating beyond 0 and 1.
    pub fn interpolate_clamped<T: Interpolate>(self, from: T, to: T) -> T {
        T::interpolate(from, to, self.0.clamp(0.0, 1.0))
    }

    /// Interpolates between values assigned to arbitrary phase coordinates.
    pub fn interpolate_between<T: Interpolate>(
        self,
        range: RangeInclusive<f32>,
        from: T,
        to: T,
    ) -> T {
        let (start, end) = range.into_inner();
        let phase = if start == end {
            if self.0 < start { 0.0 } else { 1.0 }
        } else {
            (self.0 - start) / (end - start)
        };
        T::interpolate(from, to, phase)
    }

    /// Interpolates over arbitrary phase coordinates without extrapolating.
    pub fn interpolate_between_clamped<T: Interpolate>(
        self,
        range: RangeInclusive<f32>,
        from: T,
        to: T,
    ) -> T {
        let (start, end) = range.into_inner();
        let phase = if start == end {
            if self.0 < start { 0.0 } else { 1.0 }
        } else {
            ((self.0 - start) / (end - start)).clamp(0.0, 1.0)
        };
        T::interpolate(from, to, phase)
    }
}

impl From<f32> for AnimationPhase {
    fn from(value: f32) -> Self {
        Self(value)
    }
}

impl From<bool> for AnimationPhase {
    fn from(value: bool) -> Self {
        Self(if value { 1.0 } else { 0.0 })
    }
}

impl SpringTarget for AnimationPhase {
    type Output = AnimationPhase;

    fn target(&self) -> f32 {
        self.0
    }

    fn resolve(&self, value: f32) -> Self::Output {
        Self(value)
    }
}

/// A value that supports linear interpolation and extrapolation.
pub trait Interpolate: Sized {
    /// Resolves the value at `phase`, where 0 is `from` and 1 is `to`.
    fn interpolate(from: Self, to: Self, phase: f32) -> Self;
}

impl Interpolate for f32 {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        from + (to - from) * phase
    }
}

impl Interpolate for Pixels {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        from + (to - from) * phase
    }
}

impl Interpolate for Rems {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        from + (to - from) * phase
    }
}

/// Interpolates the parts of two colors with premultiplied alpha, as CSS
/// does, so the parts of a clear color add no tint on the way. Gives the
/// parts and the alpha. A result with no alpha keeps the plain mix of the
/// parts. See <https://www.w3.org/TR/css-color-4/#interpolation-alpha>.
fn premultiplied<const N: usize>(
    (from, from_alpha): ([f32; N], f32),
    (to, to_alpha): ([f32; N], f32),
    phase: f32,
) -> ([f32; N], f32) {
    let alpha = f32::interpolate(from_alpha, to_alpha, phase);
    let parts = std::array::from_fn(|ix| {
        if alpha > 0. {
            f32::interpolate(from[ix] * from_alpha, to[ix] * to_alpha, phase) / alpha
        } else {
            f32::interpolate(from[ix], to[ix], phase)
        }
    });
    (parts, alpha)
}

impl Interpolate for Rgba {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        let ([r, g, b], a) = premultiplied(
            ([from.r, from.g, from.b], from.a),
            ([to.r, to.g, to.b], to.a),
            phase,
        );
        Self { r, g, b, a }
    }
}

/// The hue takes no alpha, as in CSS.
impl Interpolate for Hsla {
    fn interpolate(from: Self, to: Self, phase: f32) -> Self {
        let hue_delta = (to.h - from.h + 0.5).rem_euclid(1.0) - 0.5;
        let ([s, l], a) = premultiplied(([from.s, from.l], from.a), ([to.s, to.l], to.a), phase);
        Self {
            h: (from.h + hue_delta * phase).rem_euclid(1.0),
            s,
            l,
            a,
        }
    }
}

/// Controls how a spring advances and resolves its presentation value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SpringPlayback {
    /// Advances toward the latest target, preserving velocity across retargets.
    #[default]
    Running,
    /// Holds the current position and velocity until playback resumes.
    Paused,
    /// Holds the current position and discards velocity.
    Stopped,
    /// Snaps to the latest target and discards velocity.
    Completed,
    /// Returns to the initial value and discards velocity.
    Cancelled,
}

/// A stateful spring animation targeting a value or projected path.
#[derive(Clone, Debug)]
pub struct SpringAnimation<T = ()> {
    pub(crate) config: SpringConfig,
    pub(crate) target: T,
    pub(crate) epsilon: f32,
    pub(crate) initial: Option<SpringCoordinates>,
    pub(crate) playback: SpringPlayback,
}

impl SpringAnimation<()> {
    /// Creates a spring animation builder.
    pub fn new(config: SpringConfig) -> Self {
        Self {
            config,
            target: (),
            epsilon: DEFAULT_SPRING_EPSILON,
            initial: None,
            playback: SpringPlayback::Running,
        }
    }

    /// Sets the value or path targeted by this spring.
    pub fn to<T: SpringVector>(self, target: T) -> SpringAnimation<T> {
        let SpringAnimation {
            config,
            target: (),
            epsilon,
            initial,
            playback,
        } = self;
        SpringAnimation {
            config,
            target,
            epsilon,
            initial,
            playback,
        }
    }
}

impl<T> SpringAnimation<T> {
    /// Sets the settling tolerance in the coordinate space of the target.
    ///
    /// The tolerances of a target, from [`SpringVector::tolerances`], override it.
    pub fn with_epsilon(mut self, epsilon: f32) -> Self {
        self.epsilon = epsilon;
        self
    }

    /// Sets how the spring advances or resolves its current value.
    pub fn playback(mut self, playback: SpringPlayback) -> Self {
        self.playback = playback;
        self
    }
}

impl<T: SpringVector> SpringAnimation<T> {
    /// Sets the coordinates used when this element has no prior spring state.
    pub fn from(mut self, initial: T) -> Self {
        self.initial = Some(initial.coordinates());
        self
    }
}

/// Adapts a spring starting at zero with no velocity to GPUI's duration-based easing API.
///
/// The returned easing can overshoot the normalized 0..1 output range. Retargeting
/// this duration-based form restarts the spring; use [`SpringConfig::step`] when
/// preserving velocity matters.
pub fn sampled_easing(config: SpringConfig, epsilon: f32) -> (Duration, impl Fn(f32) -> f32) {
    let initial_state = SpringState {
        position: 0.0,
        velocity: 0.0,
    };
    let duration = config.settle_time(initial_state, 1.0, epsilon);
    let duration_seconds = duration.as_secs_f32();

    (duration, move |progress| {
        if progress <= 0.0 {
            0.0
        } else if progress >= 1.0 {
            1.0
        } else {
            config
                .step(initial_state, 1.0, progress * duration_seconds)
                .position
        }
    })
}

fn envelope_decay_start(constant: f32, linear: f32, decay: f32) -> f32 {
    if linear == 0.0 {
        0.0
    } else {
        (1.0 / decay - constant / linear).max(0.0)
    }
}

fn find_settle_time(
    position_threshold: f32,
    velocity_threshold: f32,
    decay_start: f32,
    natural_frequency: f32,
    envelope: impl Fn(f32) -> (f32, f32),
) -> Duration {
    let is_below_threshold = |time| {
        let (position, velocity) = envelope(time);
        position <= position_threshold && velocity <= velocity_threshold
    };

    if is_below_threshold(decay_start) {
        return duration_from_secs(decay_start);
    }

    let mut lower_bound = decay_start;
    let mut upper_bound = decay_start.max(natural_frequency.recip());
    while !is_below_threshold(upper_bound) {
        lower_bound = upper_bound;
        upper_bound *= 2.0;
        if !upper_bound.is_finite() {
            return Duration::MAX;
        }
    }

    for _ in 0..32 {
        let midpoint = (lower_bound + upper_bound) / 2.0;
        if is_below_threshold(midpoint) {
            upper_bound = midpoint;
        } else {
            lower_bound = midpoint;
        }
    }

    duration_from_secs(upper_bound)
}

fn duration_from_secs(seconds: f32) -> Duration {
    if !seconds.is_finite() || seconds >= Duration::MAX.as_secs_f32() {
        Duration::MAX
    } else if seconds <= 0.0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f32(seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-4;

    #[test]
    fn spring_targets_resolve_typed_outputs() {
        assert_eq!(12.0_f32.target(), 12.0);
        assert_eq!(12.0_f32.resolve(14.0), 14.0);
        assert_eq!(Pixels::from(12.0).target(), 12.0);
        assert_eq!(Pixels::from(12.0).resolve(14.0), Pixels::from(14.0));
        assert_eq!(false.target(), 0.0);
        assert_eq!(true.target(), 1.0);
        assert_eq!(true.resolve(1.25), AnimationPhase(1.25));
        assert_eq!(Pixels::from(12.0).coordinates().as_slice(), [12.0]);
        assert_eq!(true.resolve_coordinates(&[1.25]), AnimationPhase(1.25));
    }

    #[test]
    fn critically_damped_spring_of_no_duration_jumps() {
        let config = SpringConfig::critically_damped(Duration::ZERO);
        let away = SpringState {
            position: 0.0,
            velocity: 3.0,
        };
        for delta_time in [0.0, 0.016] {
            let state = config.step(away, 1.0, delta_time);
            assert_eq!((state.position, state.velocity), (1.0, 0.0));
            assert!(config.is_settled(state, 1.0, EPSILON));
            assert!(config.is_settled(state, 1.0, 0.0));
        }
        assert!(!config.is_settled(away, 1.0, EPSILON));
        assert_eq!(config.settle_time(away, 1.0, EPSILON), Duration::ZERO);
    }

    #[test]
    fn critically_damped_spring_comes_within_one_percent_in_its_duration() {
        let duration = Duration::from_millis(120);
        let config = SpringConfig::critically_damped(duration);
        let (_, damping_ratio) = config.canonical();
        assert!((damping_ratio - 1.0).abs() < EPSILON);

        let rest = SpringState::default();
        let at_end = config.step(rest, 1.0, duration.as_secs_f32()).position;
        assert!((at_end - 0.99).abs() < EPSILON);
    }

    #[test]
    fn animation_phases_interpolate_over_arbitrary_ranges() {
        let phase = AnimationPhase(1.5);
        assert_eq!(phase.interpolate_between(1.0..=2.0, 10.0, 20.0), 15.0);
        assert_eq!(
            phase.interpolate_between_clamped(2.0..=3.0, 10.0, 20.0),
            10.0
        );
        assert_eq!(
            AnimationPhase(3.5).interpolate_between(2.0..=3.0, 10.0, 20.0),
            25.0
        );
    }

    #[test]
    fn hsla_interpolation_takes_the_shortest_hue_path() {
        let from = Hsla {
            h: 0.9,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        };
        let to = Hsla {
            h: 0.1,
            s: 1.0,
            l: 0.75,
            a: 0.5,
        };
        let result = AnimationPhase(0.5).interpolate(from, to);

        assert!(result.h < EPSILON || (1.0 - result.h) < EPSILON);
        // The saturation and the lightness weigh by alpha.
        assert!((result.s - 0.5 / 0.75).abs() < EPSILON);
        assert!((result.l - 0.4375 / 0.75).abs() < EPSILON);
        assert!((result.a - 0.75).abs() < EPSILON);
    }

    #[test]
    fn rgba_interpolation_premultiplies_alpha() {
        let wash = Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.2,
        };
        let blue = Rgba {
            r: 0.0,
            g: 0.4,
            b: 1.0,
            a: 1.0,
        };
        let half = AnimationPhase(0.5).interpolate(wash, blue);
        // Half of 20 % white and half of the blue, over the mean alpha.
        let part = |of: f32| (0.1 + of / 2.0) / 0.6;
        for (got, want) in
            [half.r, half.g, half.b, half.a]
                .into_iter()
                .zip([part(0.0), part(0.4), part(1.0), 0.6])
        {
            assert!((got - want).abs() < EPSILON, "{half:?}");
        }

        let clear = Rgba { a: 0.0, ..wash };
        let none = AnimationPhase(0.5).interpolate(clear, Rgba { a: 0.0, ..blue });
        assert_eq!(none.a, 0.0);
        assert!((none.g - 0.7).abs() < EPSILON);
    }

    #[test]
    fn propagators_compose_and_have_expected_determinant() {
        for damping_ratio in [0.4, 1.0, 1.5] {
            let natural_frequency = 12.0;
            let config = SpringConfig::new(
                natural_frequency * natural_frequency,
                2.0 * damping_ratio * natural_frequency,
                1.0,
            );
            let first = config.propagator(0.013);
            let second = config.propagator(0.021);
            let combined = multiply(second, first);
            let direct = config.propagator(0.034);

            for row in 0..2 {
                for column in 0..2 {
                    assert!(
                        (combined[row][column] - direct[row][column]).abs() < 2e-4,
                        "{damping_ratio}: {combined:?} != {direct:?}"
                    );
                }
            }

            let determinant = direct[0][0] * direct[1][1] - direct[0][1] * direct[1][0];
            let expected = (-2.0 * damping_ratio * natural_frequency * 0.034).exp();
            assert!((determinant - expected).abs() < 2e-4);
        }
    }

    #[test]
    fn step_preserves_semigroup_for_every_damping_regime() {
        let state = SpringState {
            position: -3.0,
            velocity: 5.0,
        };
        for damping in [4.0, 20.0, 40.0] {
            let config = SpringConfig::new(100.0, damping, 1.0);
            let stepped = config.step(config.step(state, 7.0, 0.013), 7.0, 0.021);
            let direct = config.step(state, 7.0, 0.034);

            assert!((stepped.position - direct.position).abs() < 2e-4);
            assert!((stepped.velocity - direct.velocity).abs() < 2e-4);
        }
    }

    #[test]
    fn ramp_tracks_steady_state_lag() {
        let natural_frequency = 10.0;
        let damping_ratio = 0.8;
        let target_velocity = 3.0;
        let config = SpringConfig::new(
            natural_frequency * natural_frequency,
            2.0 * damping_ratio * natural_frequency,
            1.0,
        );
        let lag = -2.0 * damping_ratio * target_velocity / natural_frequency;
        let state = SpringState {
            position: lag,
            velocity: target_velocity,
        };
        let next = config.step_ramp(state, 0.0, target_velocity, 0.25);

        assert!((next.position - (target_velocity * 0.25 + lag)).abs() < EPSILON);
        assert!((next.velocity - target_velocity).abs() < EPSILON);
    }

    #[test]
    fn settling_requires_low_velocity() {
        let config = SpringConfig::new(100.0, 10.0, 1.0);
        assert!(!config.is_settled(
            SpringState {
                position: 1.0,
                velocity: 1.0,
            },
            1.0,
            0.01,
        ));
        assert!(config.is_settled(
            SpringState {
                position: 1.005,
                velocity: 0.05,
            },
            1.0,
            0.01,
        ));
    }

    #[test]
    fn settle_time_is_conservative_for_every_damping_regime() {
        let initial_state = SpringState {
            position: -2.0,
            velocity: 4.0,
        };
        for damping in [4.0, 20.0, 40.0] {
            let config = SpringConfig::new(100.0, damping, 1.0);
            let duration = config.settle_time(initial_state, 3.0, 0.001);
            assert_ne!(duration, Duration::MAX);

            for additional_time in [0.0, 0.1, 1.0] {
                let state =
                    config.step(initial_state, 3.0, duration.as_secs_f32() + additional_time);
                assert!(
                    config.is_settled(state, 3.0, 0.001),
                    "{damping}: {duration:?} produced {state:?}"
                );
            }
        }
    }

    #[test]
    fn settle_time_accounts_for_motion_outside_an_instantaneous_tolerance() {
        let config = SpringConfig::new(100.0, 2.0, 1.0);
        let state = SpringState {
            position: 1.125,
            velocity: 1.25,
        };
        assert!(config.is_settled(state, 1.0, 0.125));

        let duration = config.settle_time(state, 1.0, 0.125);
        assert!(duration > Duration::ZERO);
        assert!(config.is_settled(config.step(state, 1.0, duration.as_secs_f32()), 1.0, 0.125,));
    }

    #[test]
    fn undamped_spring_never_settles() {
        let config = SpringConfig::new(100.0, 0.0, 1.0);
        assert_eq!(
            config.settle_time(
                SpringState {
                    position: 0.0,
                    velocity: 0.0,
                },
                1.0,
                0.001,
            ),
            Duration::MAX
        );
    }

    #[test]
    fn sampled_easing_has_exact_endpoints_and_can_overshoot() {
        let config = SpringConfig::new(100.0, 6.0, 1.0);
        let (duration, easing) = sampled_easing(config, 0.001);

        assert_ne!(duration, Duration::MAX);
        assert_eq!(easing(0.0), 0.0);
        assert_eq!(easing(1.0), 1.0);
        assert!((1..100).any(|step| easing(step as f32 / 100.0) > 1.0));
    }

    fn multiply(left: [[f32; 2]; 2], right: [[f32; 2]; 2]) -> [[f32; 2]; 2] {
        [
            [
                left[0][0] * right[0][0] + left[0][1] * right[1][0],
                left[0][0] * right[0][1] + left[0][1] * right[1][1],
            ],
            [
                left[1][0] * right[0][0] + left[1][1] * right[1][0],
                left[1][0] * right[0][1] + left[1][1] * right[1][1],
            ],
        ]
    }
}
