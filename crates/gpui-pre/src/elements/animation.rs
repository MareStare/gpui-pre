use scheduler::Instant;
use std::{cell::Cell, rc::Rc, time::Duration};

use crate::{
    AnyElement, App, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    ParentElement, SpringAnimation, SpringConfig, SpringCoordinates, SpringPlayback, SpringState,
    SpringVector, Window,
};

pub use easing::*;
use smallvec::SmallVec;

/// An animation that can be applied to an element.
#[derive(Clone)]
pub struct Animation {
    /// The amount of time for which this animation should run
    pub duration: Duration,
    /// Whether to repeat this animation when it finishes
    pub oneshot: bool,
    /// Whether to derive the phase from a shared clock. See [`Animation::repeat_synced`].
    pub synced: bool,
    /// A function that maps normalized time to an animated value.
    /// The result may exceed 0..1 for easing functions that overshoot.
    pub easing: Rc<dyn Fn(f32) -> f32>,
    /// The maximum number of times per second this animation re-renders.
    /// When `None`, the animation re-renders on every frame.
    pub max_fps: Option<f32>,
}

impl Animation {
    /// Create a new animation with the given duration.
    /// By default the animation will only run once and will use a linear easing function.
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            oneshot: true,
            synced: false,
            easing: Rc::new(linear),
            max_fps: None,
        }
    }

    /// Set the animation to loop when it finishes.
    pub fn repeat(mut self) -> Self {
        self.oneshot = false;
        self
    }

    /// Set the animation to loop when it finishes, phase-locked to a clock shared by the whole [`App`].
    pub fn repeat_synced(mut self) -> Self {
        self.oneshot = false;
        self.synced = true;
        self
    }

    /// Sets the easing function used to map normalized time to an animated value.
    ///
    /// The output is not clamped, allowing physical easing functions such as
    /// springs to overshoot.
    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.easing = Rc::new(easing);
        self
    }

    /// Limit how often this animation re-renders. Instead of re-rendering on
    /// every frame, the animation schedules its next render `1 / max_fps`
    /// seconds after the current one. Values that are not finite and positive
    /// are ignored.
    pub fn with_max_fps(mut self, max_fps: f32) -> Self {
        self.max_fps = Some(max_fps);
        self
    }
}

/// An extension trait for adding the animation wrapper to both Elements and Components
///
/// Animations rendered through this trait automatically respect
/// [`App::reduce_motion`](crate::App::reduce_motion): when it is set,
/// the element is rendered in a static state (the end state for oneshot
/// animations, the start state for repeating ones) and no animation frames are
/// scheduled.
pub trait AnimationExt {
    /// Render this component or element with an animation
    fn with_animation(
        self,
        id: impl Into<ElementId>,
        animation: Animation,
        animator: impl Fn(Self, f32) -> Self + 'static,
    ) -> AnimationElement<Self>
    where
        Self: Sized,
    {
        AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: Box::new(move |this, _, value| animator(this, value)),
            animations: smallvec::smallvec![animation],
        }
    }

    /// Render this component or element with a chain of animations
    fn with_animations(
        self,
        id: impl Into<ElementId>,
        animations: Vec<Animation>,
        animator: impl Fn(Self, usize, f32) -> Self + 'static,
    ) -> AnimationElement<Self>
    where
        Self: Sized,
    {
        AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: Box::new(animator),
            animations: animations.into(),
        }
    }

    /// Renders this component or element at the value produced by springs, one
    /// per coordinate of the target.
    ///
    /// The element ID preserves position and velocity across target changes.
    /// A newly mounted spring starts at its target unless configured with
    /// [`SpringAnimation::from`].
    fn with_spring<T>(
        self,
        id: impl Into<ElementId>,
        animation: SpringAnimation<T>,
        animator: impl FnOnce(Self, T::Output) -> Self + 'static,
    ) -> SpringAnimationElement<Self>
    where
        Self: Sized,
        T: SpringVector,
        T::Output: 'static,
    {
        let SpringAnimation {
            config,
            target,
            epsilon,
            initial,
            playback,
        } = animation;
        let coordinates = target.coordinates();
        let tolerances = target
            .tolerances()
            .unwrap_or_else(|| smallvec::smallvec![epsilon; coordinates.len()]);
        assert_eq!(
            tolerances.len(),
            coordinates.len(),
            "a spring target gives one tolerance for each coordinate"
        );
        SpringAnimationElement {
            id: id.into(),
            element: Some(self),
            config,
            target: coordinates,
            tolerances,
            initial,
            playback,
            animator: Some(Box::new(move |this, coordinates| {
                animator(this, target.resolve_coordinates(coordinates))
            })),
        }
    }
}

impl<E: IntoElement + 'static> AnimationExt for E {}

/// A GPUI element that applies an animation to another element
pub struct AnimationElement<E> {
    id: ElementId,
    element: Option<E>,
    animations: SmallVec<[Animation; 1]>,
    animator: Box<dyn Fn(E, usize, f32) -> E + 'static>,
}

/// A GPUI element driven by stateful springs, one per coordinate of its target.
pub struct SpringAnimationElement<E> {
    id: ElementId,
    element: Option<E>,
    config: SpringConfig,
    target: SpringCoordinates,
    tolerances: SpringCoordinates,
    initial: Option<SpringCoordinates>,
    playback: SpringPlayback,
    animator: Option<Box<dyn FnOnce(E, &[f32]) -> E + 'static>>,
}

impl<E: ParentElement> ParentElement for SpringAnimationElement<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        let Some(element) = &mut self.element else {
            return;
        };

        element.extend(elements);
    }
}

impl<E> SpringAnimationElement<E> {
    /// Returns a new [`SpringAnimationElement<E>`] after applying the given function
    /// to the element being animated.
    pub fn map_element(mut self, f: impl FnOnce(E) -> E) -> SpringAnimationElement<E> {
        self.element = self.element.map(f);
        self
    }
}

impl<E: IntoElement + 'static> IntoElement for SpringAnimationElement<E> {
    type Element = SpringAnimationElement<E>;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: ParentElement> ParentElement for AnimationElement<E> {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        let Some(element) = &mut self.element else {
            return;
        };

        element.extend(elements);
    }
}

impl<E> AnimationElement<E> {
    /// Returns a new [`AnimationElement<E>`] after applying the given function
    /// to the element being animated.
    pub fn map_element(mut self, f: impl FnOnce(E) -> E) -> AnimationElement<E> {
        self.element = self.element.map(f);
        self
    }
}

impl<E: IntoElement + 'static> IntoElement for AnimationElement<E> {
    type Element = AnimationElement<E>;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct AnimationState {
    start: Instant,
    animation_ix: usize,
    /// Whether a throttled re-render (see [`Animation::with_max_fps`]) is
    /// already scheduled, so overlapping renders don't stack extra timers.
    delayed_frame_pending: Rc<Cell<bool>>,
}

struct SpringElementState {
    springs: SmallVec<[SpringState; 4]>,
    target: SpringCoordinates,
    config: SpringConfig,
    initial: SpringCoordinates,
    playback: SpringPlayback,
    updated_at: Instant,
}

impl SpringElementState {
    fn new(
        initial: SpringCoordinates,
        target: SpringCoordinates,
        config: SpringConfig,
        playback: SpringPlayback,
        now: Instant,
    ) -> Self {
        Self {
            springs: at_rest(&initial),
            target,
            config,
            initial,
            playback,
            updated_at: now,
        }
    }
}

fn at_rest(coordinates: &[f32]) -> SmallVec<[SpringState; 4]> {
    coordinates
        .iter()
        .map(|&position| SpringState {
            position,
            velocity: 0.0,
        })
        .collect()
}

impl<E: IntoElement + 'static> Element for SpringAnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        window.with_element_state(global_id.unwrap(), |state, window| {
            let now = cx.background_executor().now();
            let initial = self.initial.take().unwrap_or_else(|| self.target.clone());
            let mut state = state
                .filter(|state: &SpringElementState| state.springs.len() == self.target.len())
                .unwrap_or_else(|| {
                    SpringElementState::new(
                        initial,
                        self.target.clone(),
                        self.config,
                        self.playback,
                        now,
                    )
                });

            let elapsed = now.duration_since(state.updated_at).as_secs_f32();
            match state.playback {
                SpringPlayback::Running => {
                    for (spring, &target) in state.springs.iter_mut().zip(&state.target) {
                        *spring = state.config.step(*spring, target, elapsed);
                    }
                }
                SpringPlayback::Paused
                | SpringPlayback::Stopped
                | SpringPlayback::Completed
                | SpringPlayback::Cancelled => {}
            }

            state.config = self.config;
            state.target = std::mem::take(&mut self.target);

            let done = match self.playback {
                SpringPlayback::Running => {
                    if cx.reduce_motion() {
                        state.springs = at_rest(&state.target);
                        true
                    } else {
                        let mut done = true;
                        let coordinates = state.springs.iter_mut().zip(&state.target);
                        for ((spring, &target), &tolerance) in coordinates.zip(&self.tolerances) {
                            // A target that is not a number rests at once. A
                            // spring that it broke rests at the next target.
                            let broken = !(target.is_finite()
                                && spring.position.is_finite()
                                && spring.velocity.is_finite());
                            if broken || state.config.is_settled(*spring, target, tolerance) {
                                *spring = SpringState {
                                    position: target,
                                    velocity: 0.0,
                                };
                            } else {
                                done = false;
                            }
                        }
                        done
                    }
                }
                SpringPlayback::Paused => true,
                SpringPlayback::Stopped => {
                    for spring in &mut state.springs {
                        spring.velocity = 0.0;
                    }
                    true
                }
                SpringPlayback::Completed => {
                    state.springs = at_rest(&state.target);
                    true
                }
                SpringPlayback::Cancelled => {
                    state.springs = at_rest(&state.initial);
                    true
                }
            };
            state.playback = self.playback;
            state.updated_at = now;

            let element = self.element.take().expect("should only be called once");
            let animator = self.animator.take().expect("should only be called once");
            let positions: SpringCoordinates =
                state.springs.iter().map(|spring| spring.position).collect();
            let mut element = animator(element, &positions).into_any_element();

            if !done {
                window.next_frame.animating = true;
                window.request_animation_frame();
            }

            ((element.request_layout(window, cx), element), state)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

impl<E: IntoElement + 'static> Element for AnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        window.with_element_state(global_id.unwrap(), |state, window| {
            let mut state = state.unwrap_or_else(|| AnimationState {
                start: cx.background_executor().now(),
                animation_ix: 0,
                delayed_frame_pending: Rc::new(Cell::new(false)),
            });
            let (animation_ix, delta, done) = if cx.reduce_motion() {
                let animation_ix = self.animations.len() - 1;
                let delta = if self.animations[animation_ix].oneshot {
                    1.0
                } else {
                    0.0
                };
                (animation_ix, delta, true)
            } else {
                let animation_ix = state.animation_ix;
                let duration = self.animations[animation_ix].duration;

                let elapsed = if self.animations[animation_ix].synced && !duration.is_zero() {
                    let elapsed = cx.background_executor().now() - cx.synced_animation_epoch;
                    // Reduce modulo the duration before f32 conversion, which loses sub-second precision at scale.
                    Duration::from_nanos((elapsed.as_nanos() % duration.as_nanos()) as u64)
                } else {
                    cx.background_executor().now() - state.start
                };
                let mut delta = elapsed.as_secs_f32() / duration.as_secs_f32();

                let mut done = false;
                if delta > 1.0 {
                    if self.animations[animation_ix].oneshot {
                        if animation_ix >= self.animations.len() - 1 {
                            done = true;
                        } else {
                            state.start = cx.background_executor().now();
                            state.animation_ix += 1;
                        }
                        delta = 1.0;
                    } else {
                        delta %= 1.0;
                    }
                }
                (animation_ix, delta, done)
            };
            let delta = (self.animations[animation_ix].easing)(delta);

            debug_assert!(delta.is_finite(), "animated value should be finite");

            let element = self.element.take().expect("should only be called once");
            let mut element = (self.animator)(element, animation_ix, delta).into_any_element();

            if !done {
                // The animation that draws the next frame decides.
                if self.animations[state.animation_ix].oneshot {
                    window.next_frame.animating = true;
                }
                match self.animations[animation_ix].max_fps {
                    Some(max_fps) if max_fps.is_finite() && max_fps > 0.0 => {
                        if !state.delayed_frame_pending.get() {
                            state.delayed_frame_pending.set(true);
                            let delayed_frame_pending = state.delayed_frame_pending.clone();
                            let view = window.current_view();
                            let interval = Duration::from_secs_f32(1.0 / max_fps);
                            window
                                .spawn(cx, async move |cx| {
                                    cx.background_executor().timer(interval).await;
                                    delayed_frame_pending.set(false);
                                    cx.update(move |_, cx| cx.notify(view)).ok();
                                })
                                .detach();
                        }
                    }
                    _ => window.request_animation_frame(),
                }
            }

            ((element.request_layout(window, cx), element), state)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}

mod easing {
    use std::f32::consts::PI;

    /// The linear easing function, or delta itself
    pub fn linear(delta: f32) -> f32 {
        delta
    }

    /// The quadratic easing function, delta * delta
    pub fn quadratic(delta: f32) -> f32 {
        delta * delta
    }

    /// The quadratic ease-in-out function, which starts and ends slowly but speeds up in the middle
    pub fn ease_in_out(delta: f32) -> f32 {
        if delta < 0.5 {
            2.0 * delta * delta
        } else {
            let x = -2.0 * delta + 2.0;
            1.0 - x * x / 2.0
        }
    }

    /// The Quint ease-out function, which starts quickly and decelerates to a stop
    pub fn ease_out_quint() -> impl Fn(f32) -> f32 {
        move |delta| 1.0 - (1.0 - delta).powi(5)
    }

    /// Apply the given easing function, first in the forward direction and then in the reverse direction
    pub fn bounce(easing: impl Fn(f32) -> f32) -> impl Fn(f32) -> f32 {
        move |delta| {
            if delta < 0.5 {
                easing(delta * 2.0)
            } else {
                easing((1.0 - delta) * 2.0)
            }
        }
    }

    /// A custom easing function for pulsating alpha that slows down as it approaches 0.1
    pub fn pulsating_between(min: f32, max: f32) -> impl Fn(f32) -> f32 {
        let range = max - min;

        move |delta| {
            // Use a combination of sine and cubic functions for a more natural breathing rhythm
            let t = (delta * 2.0 * PI).sin();
            let breath = (t * t * t + t) / 2.0;

            // Map the breath to our desired alpha range
            let normalized_alpha = (breath + 1.0) / 2.0;

            min + (normalized_alpha * range)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, time::Duration};

    use crate::{
        Animation, Context, Entity, InteractiveElement, Pixels, Render, SpringAnimation,
        SpringConfig, StyleRefinement, TestAppContext, WindowHandle, div, prelude::*, px, size,
    };

    use super::*;

    struct AnimationTestView {
        rendered_deltas: Rc<RefCell<Vec<f32>>>,
        max_fps: Option<f32>,
    }

    struct SyncedAnimationTestView {
        show_second: bool,
        first_deltas: Rc<RefCell<Vec<f32>>>,
        second_deltas: Rc<RefCell<Vec<f32>>>,
    }

    struct SpringAnimationTestView {
        target: Pixels,
        initial: Option<Pixels>,
        playback: SpringPlayback,
        rendered_values: Rc<RefCell<Vec<Pixels>>>,
    }

    impl Render for SpringAnimationTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let rendered_values = self.rendered_values.clone();
            let mut animation = SpringAnimation::new(SpringConfig::new(100.0, 2.0, 1.0))
                .to(self.target)
                .with_epsilon(0.01)
                .playback(self.playback);
            if let Some(initial) = self.initial {
                animation = animation.from(initial);
            }
            div().with_spring("spring-animation", animation, move |this, value| {
                rendered_values.borrow_mut().push(value);
                this.left(value)
            })
        }
    }

    /// A position whose two coordinates spring on their own.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Position(f32, f32);

    impl SpringVector for Position {
        type Output = Position;

        fn coordinates(&self) -> SpringCoordinates {
            smallvec::smallvec![self.0, self.1]
        }

        fn resolve_coordinates(&self, coordinates: &[f32]) -> Self::Output {
            Position(coordinates[0], coordinates[1])
        }
    }

    struct MultiSpringTestView {
        target: Position,
        rendered: Rc<RefCell<Vec<Position>>>,
    }

    impl Render for MultiSpringTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let rendered = self.rendered.clone();
            let animation =
                SpringAnimation::new(SpringConfig::critically_damped(Duration::from_millis(200)))
                    .to(self.target)
                    .from(Position(0.0, 0.0))
                    .with_epsilon(0.01);
            div().with_spring("multi-spring", animation, move |this, position| {
                rendered.borrow_mut().push(position);
                this.left(px(position.0)).top(px(position.1))
            })
        }
    }

    /// An offset in pixels that settles within half a pixel.
    #[derive(Clone, Copy)]
    struct Offset(f32);

    impl SpringVector for Offset {
        type Output = f32;

        fn coordinates(&self) -> SpringCoordinates {
            smallvec::smallvec![self.0]
        }

        fn resolve_coordinates(&self, coordinates: &[f32]) -> Self::Output {
            coordinates[0]
        }

        fn tolerances(&self) -> Option<SpringCoordinates> {
            Some(smallvec::smallvec![0.5])
        }
    }

    /// A glide of 300 pixels in 120 ms, with the tolerance of the offset or
    /// with the default epsilon.
    struct GlideTestView {
        to: f32,
        in_pixels: bool,
    }

    impl Render for GlideTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let config = SpringConfig::critically_damped(Duration::from_millis(120));
            let el = div();
            if self.in_pixels {
                let animation = SpringAnimation::new(config)
                    .to(Offset(self.to))
                    .from(Offset(0.0));
                el.with_spring("glide", animation, |this, x| this.left(px(x)))
                    .into_any_element()
            } else {
                let animation = SpringAnimation::new(config).to(self.to).from(0.0);
                el.with_spring("glide", animation, |this, x| this.left(px(x)))
                    .into_any_element()
            }
        }
    }

    impl Render for SyncedAnimationTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let record_deltas = |deltas: Rc<RefCell<Vec<f32>>>| {
                move |this, delta| {
                    deltas.borrow_mut().push(delta);
                    this
                }
            };
            div()
                .size_full()
                .child(div().with_animation(
                    "first-synced-animation",
                    Animation::new(Duration::from_secs(1)).repeat_synced(),
                    record_deltas(self.first_deltas.clone()),
                ))
                .when(self.show_second, |this| {
                    this.child(div().with_animation(
                        "second-synced-animation",
                        Animation::new(Duration::from_secs(1)).repeat_synced(),
                        record_deltas(self.second_deltas.clone()),
                    ))
                })
        }
    }

    /// A one-shot animation beside a repeating one.
    struct OneShotTestView;

    impl Render for OneShotTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().with_animation(
                    "one-shot-animation",
                    Animation::new(Duration::from_secs(1)),
                    |this, _| this,
                ))
                .child(div().with_animation(
                    "repeating-animation",
                    Animation::new(Duration::from_secs(1)).repeat(),
                    |this, _| this,
                ))
        }
    }

    /// A one-shot intro that a repeating animation follows.
    struct IntroTestView;

    impl Render for IntroTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(div().with_animations(
                "intro-animation",
                vec![
                    Animation::new(Duration::from_secs(1)),
                    Animation::new(Duration::from_secs(1)).repeat(),
                ],
                |this, _, _| this,
            ))
        }
    }

    /// A cached view with a one-shot animation at a low frame rate, in a
    /// view that draws on its own.
    struct CachedOneShotTestView {
        child: Entity<ThrottledOneShotView>,
    }

    impl Render for CachedOneShotTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                self.child
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }

    struct ThrottledOneShotView;

    impl Render for ThrottledOneShotView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().with_animation(
                "throttled-one-shot-animation",
                Animation::new(Duration::from_secs(1)).with_max_fps(2.0),
                |this, _| this,
            )
        }
    }

    impl Render for AnimationTestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let rendered_deltas = self.rendered_deltas.clone();
            // The throttled variant syncs to the shared clock so the deltas
            // follow the test scheduler's clock rather than wall time.
            let mut animation = Animation::new(Duration::from_secs(1));
            if let Some(max_fps) = self.max_fps {
                animation = animation.repeat_synced().with_max_fps(max_fps);
            } else {
                animation = animation.repeat();
            }
            div().size_full().child(div().with_animation(
                "repeating-animation",
                animation,
                move |this, delta| {
                    rendered_deltas.borrow_mut().push(delta);
                    this
                },
            ))
        }
    }

    fn open_test_window(
        cx: &mut TestAppContext,
    ) -> (Rc<RefCell<Vec<f32>>>, WindowHandle<AnimationTestView>) {
        open_test_window_with_max_fps(cx, None)
    }

    fn open_test_window_with_max_fps(
        cx: &mut TestAppContext,
        max_fps: Option<f32>,
    ) -> (Rc<RefCell<Vec<f32>>>, WindowHandle<AnimationTestView>) {
        let rendered_deltas = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.), px(100.)), {
            let rendered_deltas = rendered_deltas.clone();
            move |_, _| AnimationTestView {
                rendered_deltas,
                max_fps,
            }
        });
        cx.run_until_parked();
        (rendered_deltas, window)
    }

    fn animating<V: 'static>(window: &WindowHandle<V>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |_, window, _| window.animating())
            .unwrap()
    }

    fn simulate_next_frame<V: Render>(window: &WindowHandle<V>, cx: &mut TestAppContext) -> usize {
        let callback_count = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        cx.run_until_parked();
        callback_count
    }
    // Before parent-animation-element, using .with_animation
    // would not allow chaining .parent after. This is just a
    // build check that we can call div().id().with_animation().child()
    #[test]
    fn test_animation_parent() {
        div()
            .id("id")
            //
            .with_animation(
                "animation",
                Animation::new(Duration::from_secs(1)),
                |el, _t| {
                    //
                    el
                },
            )
            .child(
                //
                div(),
            );
    }

    #[test]
    fn test_spring_animation_parent() {
        div()
            .id("id")
            .with_spring(
                "spring-animation",
                SpringAnimation::new(SpringConfig::new(100.0, 10.0, 1.0))
                    .to(px(10.0))
                    .from(px(0.0)),
                |element, value| element.left(value),
            )
            .child(div());
    }

    #[gpui::test]
    fn test_spring_animation_preserves_velocity_when_retargeted(cx: &mut TestAppContext) {
        let rendered_values = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.0), px(100.0)), {
            let rendered_values = rendered_values.clone();
            move |_, _| SpringAnimationTestView {
                target: px(0.0),
                initial: None,
                playback: SpringPlayback::Running,
                rendered_values,
            }
        });
        cx.run_until_parked();
        assert_eq!(*rendered_values.borrow(), vec![px(0.0)]);

        window
            .update(cx, |view, _, cx| {
                view.target = px(100.0);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();

        cx.executor().advance_clock(Duration::from_millis(50));
        assert!(simulate_next_frame(&window, cx) > 0);
        let value_before_retargeting = *rendered_values.borrow().last().unwrap();
        assert!(value_before_retargeting > px(0.0));
        assert!(value_before_retargeting < px(100.0));

        window
            .update(cx, |view, _, cx| {
                view.target = px(0.0);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();

        cx.executor().advance_clock(Duration::from_millis(5));
        assert!(simulate_next_frame(&window, cx) > 0);
        let value_after_retargeting = *rendered_values.borrow().last().unwrap();
        assert!(value_after_retargeting > value_before_retargeting);
    }

    #[gpui::test]
    fn test_each_coordinate_springs_on_its_own(cx: &mut TestAppContext) {
        let rendered = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.0), px(100.0)), {
            let rendered = rendered.clone();
            move |_, _| MultiSpringTestView {
                target: Position(100.0, -50.0),
                rendered,
            }
        });
        cx.run_until_parked();
        assert_eq!(*rendered.borrow(), vec![Position(0.0, 0.0)]);
        assert!(
            animating(&window, cx),
            "a moving spring is an animation of the frame"
        );

        cx.executor().advance_clock(Duration::from_millis(50));
        assert!(simulate_next_frame(&window, cx) > 0);
        let midway = *rendered.borrow().last().unwrap();
        assert!(0.0 < midway.0 && midway.0 < 100.0, "{midway:?}");
        // Both coordinates cover the same share of their distance.
        assert!(
            (midway.0 / 100.0 - midway.1 / -50.0).abs() < 1e-4,
            "{midway:?}"
        );

        // The first coordinate turns back with its velocity. The second one
        // keeps its target and its course.
        window
            .update(cx, |view, _, cx| {
                view.target = Position(0.0, -50.0);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(5));
        assert!(simulate_next_frame(&window, cx) > 0);
        let turned = *rendered.borrow().last().unwrap();
        assert!(turned.0 > midway.0, "{turned:?}");
        assert!(turned.1 < midway.1, "{turned:?}");

        for _ in 0..100 {
            cx.executor().advance_clock(Duration::from_millis(16));
            if simulate_next_frame(&window, cx) == 0 {
                break;
            }
        }
        assert_eq!(*rendered.borrow().last().unwrap(), Position(0.0, -50.0));
        assert_eq!(simulate_next_frame(&window, cx), 0);
        assert!(!animating(&window, cx));
    }

    #[gpui::test]
    fn test_a_glide_in_pixels_settles_near_its_duration(cx: &mut TestAppContext) {
        let mut glide = |in_pixels| {
            let window = cx.open_window(size(px(400.0), px(100.0)), |_, _| GlideTestView {
                to: 300.0,
                in_pixels,
            });
            cx.run_until_parked();
            let mut elapsed = Duration::ZERO;
            while elapsed < Duration::from_secs(1) {
                cx.executor().advance_clock(Duration::from_millis(8));
                elapsed += Duration::from_millis(8);
                if simulate_next_frame(&window, cx) == 0 {
                    break;
                }
            }
            elapsed
        };
        let in_pixels = glide(true);
        assert!(in_pixels <= Duration::from_millis(180), "{in_pixels:?}");
        // The default epsilon is a thousandth of a pixel here.
        let fine = glide(false);
        assert!(fine >= Duration::from_millis(240), "{fine:?}");
    }

    #[gpui::test]
    fn test_paused_spring_resumes_with_its_velocity(cx: &mut TestAppContext) {
        let rendered_values = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.0), px(100.0)), {
            let rendered_values = rendered_values.clone();
            move |_, _| SpringAnimationTestView {
                target: px(0.0),
                initial: None,
                playback: SpringPlayback::Running,
                rendered_values,
            }
        });
        cx.run_until_parked();

        window
            .update(cx, |view, _, cx| {
                view.target = px(100.0);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(50));
        assert!(simulate_next_frame(&window, cx) > 0);

        window
            .update(cx, |view, _, cx| {
                view.target = px(0.0);
                view.playback = SpringPlayback::Paused;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let paused_value = *rendered_values.borrow().last().unwrap();

        cx.executor().advance_clock(Duration::from_millis(500));
        assert!(simulate_next_frame(&window, cx) > 0);
        assert_eq!(*rendered_values.borrow().last().unwrap(), paused_value);
        assert_eq!(simulate_next_frame(&window, cx), 0);

        window
            .update(cx, |view, _, cx| {
                view.playback = SpringPlayback::Running;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(5));
        assert!(simulate_next_frame(&window, cx) > 0);
        assert!(*rendered_values.borrow().last().unwrap() > paused_value);
    }

    #[gpui::test]
    fn test_stopped_spring_resumes_without_velocity(cx: &mut TestAppContext) {
        let rendered_values = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.0), px(100.0)), {
            let rendered_values = rendered_values.clone();
            move |_, _| SpringAnimationTestView {
                target: px(0.0),
                initial: None,
                playback: SpringPlayback::Running,
                rendered_values,
            }
        });
        cx.run_until_parked();

        window
            .update(cx, |view, _, cx| {
                view.target = px(1_000_000.0);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(50));
        assert!(simulate_next_frame(&window, cx) > 0);

        window
            .update(cx, |view, _, cx| {
                view.target = px(0.0);
                view.playback = SpringPlayback::Stopped;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        let stopped_value = *rendered_values.borrow().last().unwrap();

        cx.executor().advance_clock(Duration::from_millis(500));
        assert!(simulate_next_frame(&window, cx) > 0);
        assert_eq!(*rendered_values.borrow().last().unwrap(), stopped_value);
        assert_eq!(simulate_next_frame(&window, cx), 0);

        window
            .update(cx, |view, _, cx| {
                view.target = stopped_value;
                view.playback = SpringPlayback::Running;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(*rendered_values.borrow().last().unwrap(), stopped_value);
        assert_eq!(simulate_next_frame(&window, cx), 0);
    }

    #[gpui::test]
    fn test_cancelled_and_completed_springs_resolve_their_endpoints(cx: &mut TestAppContext) {
        let rendered_values = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.0), px(100.0)), {
            let rendered_values = rendered_values.clone();
            move |_, _| SpringAnimationTestView {
                target: px(100.0),
                initial: Some(px(20.0)),
                playback: SpringPlayback::Running,
                rendered_values,
            }
        });
        cx.run_until_parked();
        assert_eq!(*rendered_values.borrow(), vec![px(20.0)]);

        cx.executor().advance_clock(Duration::from_millis(50));
        assert!(simulate_next_frame(&window, cx) > 0);
        assert!(*rendered_values.borrow().last().unwrap() > px(20.0));

        window
            .update(cx, |view, _, cx| {
                view.playback = SpringPlayback::Cancelled;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(*rendered_values.borrow().last().unwrap(), px(20.0));
        assert!(simulate_next_frame(&window, cx) > 0);
        assert_eq!(simulate_next_frame(&window, cx), 0);

        window
            .update(cx, |view, _, cx| {
                view.playback = SpringPlayback::Completed;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(*rendered_values.borrow().last().unwrap(), px(100.0));
        assert_eq!(simulate_next_frame(&window, cx), 0);
    }

    #[gpui::test]
    fn test_spring_animation_rests_at_a_target_that_is_not_a_number(cx: &mut TestAppContext) {
        let rendered_values = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.0), px(100.0)), {
            let rendered_values = rendered_values.clone();
            move |_, _| SpringAnimationTestView {
                target: px(0.0),
                initial: None,
                playback: SpringPlayback::Running,
                rendered_values,
            }
        });
        cx.run_until_parked();
        let retarget = |target: Pixels, cx: &mut TestAppContext| {
            window
                .update(cx, |view, _, cx| {
                    view.target = target;
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(16));
            let frames = simulate_next_frame(&window, cx);
            (*rendered_values.borrow().last().unwrap(), frames)
        };

        let (value, frames) = retarget(px(f32::NAN), cx);
        assert!(value.0.is_nan());
        assert_eq!(frames, 0, "a target that is not a number draws no frames");
        assert_eq!(retarget(px(50.0), cx), (px(50.0), 0));
    }

    #[gpui::test]
    fn test_spring_animation_respects_reduced_motion(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let rendered_values = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.0), px(100.0)), {
            let rendered_values = rendered_values.clone();
            move |_, _| SpringAnimationTestView {
                target: px(100.0),
                initial: None,
                playback: SpringPlayback::Running,
                rendered_values,
            }
        });
        cx.run_until_parked();

        assert_eq!(*rendered_values.borrow(), vec![px(100.0)]);
        assert_eq!(simulate_next_frame(&window, cx), 0);
    }

    #[gpui::test]
    fn test_repeating_animation_schedules_animation_frames(cx: &mut TestAppContext) {
        let (rendered_deltas, window) = open_test_window(cx);

        assert_eq!(rendered_deltas.borrow().len(), 1);

        for expected_frames in 2..=3 {
            assert_eq!(simulate_next_frame(&window, cx), 1);
            assert_eq!(rendered_deltas.borrow().len(), expected_frames);
        }
    }

    #[gpui::test]
    fn test_max_fps_schedules_timer_driven_frames(cx: &mut TestAppContext) {
        let (rendered_deltas, window) = open_test_window_with_max_fps(cx, Some(10.0));

        // The test scheduler's clock jitters forward slightly on each poll,
        // so compare against expectations loosely.
        let assert_deltas_approx_eq = |expected: &[f32]| {
            let actual = rendered_deltas.borrow();
            assert_eq!(actual.len(), expected.len(), "deltas: {actual:?}");
            for (actual, expected) in actual.iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 1e-2,
                    "expected {expected}, got {actual}"
                );
            }
        };

        assert_deltas_approx_eq(&[0.0]);

        // No per-frame callback is scheduled; re-renders are timer-driven.
        assert_eq!(simulate_next_frame(&window, cx), 0);
        assert_deltas_approx_eq(&[0.0]);

        cx.executor().advance_clock(Duration::from_millis(105));
        cx.run_until_parked();
        assert_deltas_approx_eq(&[0.0, 0.105]);

        cx.executor().advance_clock(Duration::from_millis(105));
        cx.run_until_parked();
        assert_deltas_approx_eq(&[0.0, 0.105, 0.21]);
    }

    #[gpui::test]
    fn test_synced_animations_share_phase_across_elements(cx: &mut TestAppContext) {
        let first_deltas = Rc::new(RefCell::new(Vec::new()));
        let second_deltas = Rc::new(RefCell::new(Vec::new()));
        let window = cx.open_window(size(px(100.), px(100.)), {
            let first_deltas = first_deltas.clone();
            let second_deltas = second_deltas.clone();
            move |_, _| SyncedAnimationTestView {
                show_second: false,
                first_deltas,
                second_deltas,
            }
        });
        cx.run_until_parked();

        assert_eq!(*first_deltas.borrow(), vec![0.0]);

        cx.executor().advance_clock(Duration::from_millis(250));
        simulate_next_frame(&window, cx);
        assert_eq!(*first_deltas.borrow(), vec![0.0, 0.25]);

        // The second element mounts a quarter through the cycle, yet renders
        // the shared phase rather than starting at zero.
        window
            .update(cx, |view, _, cx| {
                view.show_second = true;
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(250));
        simulate_next_frame(&window, cx);

        assert_eq!(*second_deltas.borrow().last().unwrap(), 0.5);
        assert_eq!(
            *first_deltas.borrow().last().unwrap(),
            *second_deltas.borrow().last().unwrap()
        );
        assert!(second_deltas.borrow().iter().all(|delta| *delta > 0.0));

        // The phase wraps around each full cycle.
        cx.executor().advance_clock(Duration::from_millis(2250));
        simulate_next_frame(&window, cx);
        assert_eq!(*first_deltas.borrow().last().unwrap(), 0.75);

        // Sub-second precision survives months of uptime: converting the raw
        // elapsed time to f32 would round 0.25 away entirely.
        cx.executor()
            .advance_clock(Duration::from_secs(300 * 24 * 60 * 60) + Duration::from_millis(500));
        simulate_next_frame(&window, cx);
        assert_eq!(*first_deltas.borrow().last().unwrap(), 0.25);
    }

    #[gpui::test]
    fn test_a_frame_is_animating_during_a_one_shot_intro(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(100.), px(100.)), |_, _| IntroTestView);
        cx.run_until_parked();
        assert!(animating(&window, cx));

        cx.executor().advance_clock(Duration::from_millis(1100));
        simulate_next_frame(&window, cx);
        assert!(
            !animating(&window, cx),
            "the repeat after the intro does not count"
        );
    }

    #[gpui::test]
    fn test_a_cached_view_keeps_its_one_shot_animation_animating(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(100.), px(100.)), |_, cx| CachedOneShotTestView {
            child: cx.new(|_| ThrottledOneShotView),
        });
        cx.run_until_parked();
        assert!(animating(&window, cx));

        // The outer view draws again before the timer of the frame rate
        // fires. The cached view draws from its cache.
        cx.executor().advance_clock(Duration::from_millis(100));
        window.update(cx, |_, _, cx| cx.notify()).unwrap();
        cx.run_until_parked();
        assert!(animating(&window, cx));

        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        simulate_next_frame(&window, cx);
        assert!(!animating(&window, cx));
    }

    #[gpui::test]
    fn test_a_frame_is_animating_until_its_one_shot_animations_end(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(100.), px(100.)), |_, _| OneShotTestView);
        cx.run_until_parked();
        assert!(animating(&window, cx));

        cx.executor().advance_clock(Duration::from_millis(500));
        simulate_next_frame(&window, cx);
        assert!(animating(&window, cx));

        cx.executor().advance_clock(Duration::from_millis(600));
        simulate_next_frame(&window, cx);
        assert!(!animating(&window, cx));
        assert!(
            simulate_next_frame(&window, cx) > 0,
            "the repeating animation asks for frames"
        );
        assert!(!animating(&window, cx));
    }

    #[gpui::test]
    fn test_reduce_motion_renders_single_static_frame(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (rendered_deltas, window) = open_test_window(cx);

        assert_eq!(*rendered_deltas.borrow(), vec![0.0]);

        assert_eq!(simulate_next_frame(&window, cx), 0);
        assert_eq!(*rendered_deltas.borrow(), vec![0.0]);
    }
}
