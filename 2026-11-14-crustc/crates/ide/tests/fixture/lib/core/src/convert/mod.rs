pub trait Into<T>: Sized {
    fn into(self) -> T;
}

pub trait From<T>: Sized {
    fn from(value: T) -> Self;
}

impl<T, U> Into<U> for T
where
    U: From<T>,
{
    fn into(self) -> U {
        loop {}
    }
}
