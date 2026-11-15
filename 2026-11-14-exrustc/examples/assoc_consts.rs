// 関連定数のサンプル: impl の const (`S::N` / `Self::N`)、trait の const (既定値あり・なし)、
// ジェネリック関数の中の `T::K`、const を配列の長さや別の const に使う
struct Buf;

impl Buf {
    const CAP: usize = 4;
    const NAME: &'static str = "buf";
    fn cap2(&self) -> usize {
        Self::CAP * 2
    }
}

trait Shape {
    const SIDES: u32;
    const LABEL: &'static str = "shape";
    fn describe(&self) -> String {
        format!("{} with {} sides", Self::LABEL, Self::SIDES)
    }
}

struct Tri;
struct Sq;

impl Shape for Tri {
    const SIDES: u32 = 3;
}

impl Shape for Sq {
    const SIDES: u32 = 4;
    const LABEL: &'static str = "square";
}

fn total_sides<T: Shape>(n: u32) -> u32 {
    T::SIDES * n
}

const LIMIT: usize = Buf::CAP + 1;

fn main() {
    let arr = [0u8; Buf::CAP];
    println!("{} {} {} {} {}", Buf::CAP, Buf::NAME, Buf.cap2(), arr.len(), LIMIT);
    println!("{} | {}", Tri.describe(), Sq.describe());
    println!("{} {} {}", Tri::SIDES + Sq::SIDES, total_sides::<Sq>(3), total_sides::<Tri>(2));
    println!("{} {}", i32::MAX - 1, u8::BITS);
}
