pub struct Enumerate<I> {
    iter: I,
    count: usize,
}

impl<I> Iterator for Enumerate<I>
where
    I: Iterator,
{
    type Item = (usize, <I as Iterator>::Item);
    fn next(&mut self) -> Option<(usize, <I as Iterator>::Item)> {
        loop {}
    }
}
