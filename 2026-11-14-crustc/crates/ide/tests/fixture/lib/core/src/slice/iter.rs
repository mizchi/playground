pub struct Iter<'a, T: 'a> {
    ptr: *const T,
    end: *const T,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        loop {}
    }
}
