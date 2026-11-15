// macro_rules! のサンプル: 式の断片 (優先順位を保つ)、繰り返し $(..),* / $(..);* / $(..)+、再帰、
// ident / ty / literal / tt / block の断片、アイテムを作るマクロ (型ごとの impl・関数)、マップのリテラル、
// 定義の前にある呼び出しと、マクロの中からの println!
use std::collections::HashMap;

macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}

macro_rules! max_of {
    ($a:expr) => { $a };
    ($a:expr, $($rest:expr),+) => {{
        let r = max_of!($($rest),+);
        if $a > r { $a } else { r }
    }};
}

macro_rules! count {
    () => { 0usize };
    ($head:tt $($tail:tt)*) => { 1usize + count!($($tail)*) };
}

macro_rules! hashmap {
    ($($k:expr => $v:expr),* $(,)?) => {{
        let mut m = HashMap::new();
        $( m.insert($k, $v); )*
        m
    }};
}

trait Describe {
    fn describe(&self) -> String;
}

macro_rules! impl_describe {
    ($($t:ty => $name:literal),*) => {
        $(
            impl Describe for $t {
                fn describe(&self) -> String {
                    format!("{} {}", $name, self)
                }
            }
        )*
    };
}

impl_describe!(i32 => "int", u8 => "byte", bool => "flag");

macro_rules! make_getter {
    ($fname:ident, $val:expr) => {
        fn $fname() -> i64 {
            $val
        }
    };
}

make_getter!(forty_two, 40 + 2);
make_getter!(seven, 7);

macro_rules! log {
    ($lvl:ident: $($arg:tt)*) => {
        println!("[{}] {}", stringify_lvl!($lvl), format!($($arg)*))
    };
}

macro_rules! stringify_lvl {
    (info) => { "INFO" };
    (warn) => { "WARN" };
}

macro_rules! twice {
    ($b:block) => {{
        $b;
        $b
    }};
}

fn main() {
    println!("{} {} {}", square!(7), square!(1 + 2), square!(2) + 1);
    println!("{} {}", max_of!(3), max_of!(4, 9, 2, 7));
    println!("{} {}", count!(a b c d), count!());
    let m = hashmap! { "a" => 1, "b" => 2, "c" => 3, };
    let mut ks: Vec<_> = m.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
    ks.sort();
    println!("{:?}", ks);
    println!("{} | {} | {}", 5.describe(), 7u8.describe(), true.describe());
    println!("{} {}", forty_two(), seven());
    log!(info: "started {} workers", 4);
    log!(warn: "low disk: {}%", 93);
    let mut n = 0;
    let r = twice!({
        n += 1;
        n * 10
    });
    println!("{} {}", n, r);
}
