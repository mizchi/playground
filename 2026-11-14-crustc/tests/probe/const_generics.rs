fn len_of<T, const N: usize>(_a: &[T; N]) -> usize {
    N
}
fn zeros<const N: usize>() -> [i32; N] {
    [0; N]
}
fn sum<const N: usize>(a: [i64; N]) -> i64 {
    let mut s = 0;
    for i in 0..N {
        s += a[i];
    }
    s
}
fn first_n<const N: usize>(v: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N && i < v.len() {
        out[i] = v[i];
        i += 1;
    }
    out
}
struct Buf<const N: usize> {
    data: [u8; N],
    len: usize,
}
impl<const N: usize> Buf<N> {
    fn new() -> Self {
        Buf { data: [0; N], len: 0 }
    }
    fn push(&mut self, b: u8) -> bool {
        if self.len == N {
            return false;
        }
        self.data[self.len] = b;
        self.len += 1;
        true
    }
    fn capacity(&self) -> usize {
        N
    }
}
fn main() {
    println!("{}", len_of(&[1, 2, 3]));
    println!("{:?}", zeros::<4>());
    let z: [i32; 2] = zeros();
    println!("{:?}", z);
    println!("{}", sum([1, 2, 3, 4]));
    let f: [u8; 3] = first_n(b"hello");
    println!("{:?} {:?}", f, first_n::<2>(b"xyz"));
    let mut b: Buf<2> = Buf::new();
    println!("{} {} {} {}", b.push(1), b.push(2), b.push(3), b.capacity());
    println!("{:?} {}", b.data, b.len);
    let b8 = Buf::<8>::new();
    println!("{} {}", b8.capacity(), std::mem::size_of::<Buf<8>>());
    const K: usize = 3;
    println!("{:?}", zeros::<K>());
    println!("{}", N_TIMES * 2);
}
const N_TIMES: usize = 5;
