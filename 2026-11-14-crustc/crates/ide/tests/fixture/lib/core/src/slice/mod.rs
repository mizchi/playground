pub mod iter;
pub use iter::Iter;

impl<T> [T] {
    pub const fn len(&self) -> usize {
        loop {}
    }
    pub fn iter(&self) -> Iter<'_, T> {
        loop {}
    }
    pub const fn first(&self) -> Option<&T> {
        loop {}
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        loop {}
    }
}
