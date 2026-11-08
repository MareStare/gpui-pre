mod dispatcher;
mod display;
mod platform;
mod window;

pub use dispatcher::*;
pub(crate) use display::*;
#[cfg(any(test, feature = "headless"))]
pub use platform::headless_platform;
pub(crate) use platform::*;
pub(crate) use window::*;

#[cfg(any(test, feature = "test-support"))]
pub use platform::{TestScreenCaptureSource, TestScreenCaptureStream};
