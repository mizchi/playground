/// A contiguous growable array type.
pub struct Vec<T> {
    buf: RawVec<T>,
    len: usize,
}

impl<T> Vec<T> {
    pub const fn new() -> Self {
        loop {}
    }
    pub fn push(&mut self, value: T) {
        loop {}
    }
    pub fn pop(&mut self) -> Option<T> {
        loop {}
    }
    pub const fn len(&self) -> usize {
        loop {}
    }
    pub const fn is_empty(&self) -> bool {
        loop {}
    }
}

impl<T> Deref for Vec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        loop {}
    }
}

impl<T: Clone> Clone for Vec<T> {
    fn clone(&self) -> Self {
        loop {}
    }
}
