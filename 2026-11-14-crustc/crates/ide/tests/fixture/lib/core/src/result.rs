//! Error handling with the `Result` type.

/// `Result` is a type that represents either success (`Ok`) or failure (`Err`).
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

impl<T, E> Result<T, E> {
    pub fn unwrap(self) -> T {
        loop {}
    }
    pub fn ok(self) -> Option<T> {
        loop {}
    }
    pub fn map_err<F, O: FnOnce(E) -> F>(self, op: O) -> Result<T, F> {
        loop {}
    }
    pub const fn is_ok(&self) -> bool {
        loop {}
    }
}
