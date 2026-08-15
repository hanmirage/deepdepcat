//! Intent classification — structured understanding of user messages.

pub mod classify;
pub mod route;
pub mod signals;
pub mod spec;
mod types;
mod text;
pub use types::*;
#[cfg(test)]
pub(crate) use text::*;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_signal;

pub use classify::*;
pub use route::*;
pub use signals::*;
pub use spec::*;
