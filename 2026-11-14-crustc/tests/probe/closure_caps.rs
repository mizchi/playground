// クロージャのキャプチャ: 値として渡す変数と代入する変数が混ざるとき (FnMut)、
// キャプチャした `&Vec<T>` を `&[T]` の引数に渡す、`&mut dyn FnMut` を受け渡して中で代入する、
// matches! の guard の中のキャプチャ
fn cnt(caps: &[String]) -> usize { caps.len() }
fn each(v: &[i32], f: &mut dyn FnMut(&i32)) { for x in v { f(x); } }
fn twice(v: &[i32], f: &mut dyn FnMut(&i32)) { each(v, f); each(v, f); }

fn found_in(v: &[i32], caps: &[String], depth: u32) -> bool {
    if depth == 0 {
        return caps.iter().any(|c| c == "a");
    }
    let mut found = false;
    each(v, &mut |_x| {
        if !found && found_in(v, caps, depth - 1) {
            found = true;
        }
    });
    found
}

fn main() {
    let caps = vec!["a".to_string(), "b".to_string()];
    let v = &caps;
    let mut r = 0;
    let mut g = || { r += 1; r += cnt(v); };
    g();
    g();
    let h = || cnt(v) * 10;
    println!("{r} {}", h());
    let mut s = 0;
    twice(&[1, 2, 3], &mut |x| s += *x);
    println!("{s} {} {}", found_in(&[1], &caps, 1), found_in(&[1, 2], &caps, 3));
    let ks = [3, 7, 9];
    let lim = 5;
    let big: Vec<_> = ks.iter().filter(|k| matches!(k, x if **x > lim)).collect();
    println!("{:?}", big);
}
