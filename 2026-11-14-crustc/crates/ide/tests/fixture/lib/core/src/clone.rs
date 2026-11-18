/// A common trait for the ability to explicitly duplicate an object.
pub trait Clone: Sized {
    fn clone(&self) -> Self;
}
