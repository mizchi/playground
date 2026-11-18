//! Optional values.

/// The `Option` type.
pub enum Option<T> {
    /// No value.
    None,
    /// Some value of type `T`.
    Some(T),
}

pub use Option::{None, Some};

impl<T> Option<T> {
    /// Returns `true` if the option is a `Some` value.
    pub const fn is_some(&self) -> bool {
        matches!(*self, Some(_))
    }
    pub fn unwrap(self) -> T {
        loop {}
    }
    pub fn unwrap_or(self, default: T) -> T {
        loop {}
    }
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Option<U> {
        loop {}
    }
    pub const fn as_ref(&self) -> Option<&T> {
        loop {}
    }
}
