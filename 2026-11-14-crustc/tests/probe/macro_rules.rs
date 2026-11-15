macro_rules! sq {
    ($x:expr) => {
        $x * $x
    };
}
macro_rules! maxm { ($a:expr) => { $a }; ($a:expr, $($r:expr),+) => { { let b = maxm!($($r),+); if $a > b { $a } else { b } } }; }
fn main() {
    println!("{} {}", sq!(7), maxm!(3, 9, 4));
}
