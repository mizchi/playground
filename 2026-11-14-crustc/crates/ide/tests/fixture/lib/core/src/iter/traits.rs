pub trait Iterator {
    type Item;
    fn next(&mut self) -> Option<Self::Item>;
    fn count(self) -> usize
    where
        Self: Sized,
    {
        loop {}
    }
    fn enumerate(self) -> Enumerate<Self>
    where
        Self: Sized,
    {
        loop {}
    }
    fn last(self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        loop {}
    }
}
