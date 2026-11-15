// クロージャのサンプル: キャプチャ (参照 / move)、Fn / FnMut / FnOnce 境界、impl Fn (引数・戻り値)、
// Option / Result のクロージャを取るメソッド、struct に入れたクロージャ、入れ子のクロージャ
unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}

fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}

fn print_i64(n: i64) {
    if n < 0 {
        put(b'-');
        print_i64(0 - n);
        return;
    }
    if n >= 10 {
        print_i64(n / 10);
    }
    put(b'0' + (n % 10) as u8);
}

fn line(n: i64) {
    print_i64(n);
    put(b'\n');
}

fn apply<F: Fn(i64) -> i64>(f: F, x: i64) -> i64 {
    f(x)
}

fn twice(f: impl Fn(i64) -> i64, x: i64) -> i64 {
    f(f(x))
}

fn each<F>(xs: &[i64], mut f: F)
where
    F: FnMut(i64),
{
    for &x in xs {
        f(x);
    }
}

fn fold<T: Copy, A, F: Fn(A, T) -> A>(xs: &[T], init: A, f: F) -> A {
    let mut acc = init;
    for &x in xs {
        acc = f(acc, x);
    }
    acc
}

fn consume<F: FnOnce() -> i64>(f: F) -> i64 {
    f()
}

fn make_adder(n: i64) -> impl Fn(i64) -> i64 {
    move |x| x + n
}

fn compose<F: Fn(i64) -> i64, G: Fn(i64) -> i64>(f: F, g: G, x: i64) -> i64 {
    g(f(x))
}

fn by_ref<F: Fn(i64) -> bool>(f: &F, x: i64) -> bool {
    f(x)
}

fn count_if(xs: &[i64], pred: &dyn Fn(i64) -> bool) -> i64 {
    let mut n = 0;
    for &x in xs {
        if pred(x) {
            n += 1;
        }
    }
    n
}

struct Button<F: Fn(i64) -> i64> {
    id: i64,
    on_click: F,
}

impl<F: Fn(i64) -> i64> Button<F> {
    fn click(&self) -> i64 {
        (self.on_click)(self.id)
    }
}

struct Counter {
    n: i64,
}

impl Counter {
    fn bump_by(&mut self, k: i64) {
        self.n += k;
    }
}

fn main() {
    // 参照キャプチャ (型は使われ方から推論する)
    let k = 10;
    let add_k = |x| x + k;
    line(apply(add_k, 5));
    line(apply(|x| x * 3, 7));
    line(twice(|x: i64| x - 1, 0));

    // 複数引数・注釈つき戻り値
    let add = |a: i64, b: i64| -> i64 { a + b };
    line(add(2, 40));

    // FnMut: 外の変数を書き換える
    let mut sum = 0;
    each(&[1, 2, 3, 4], |x| sum += x);
    line(sum);
    let mut count = 0;
    let mut inc = || count += 1;
    inc();
    inc();
    inc();
    line(count);

    // ジェネリックな畳み込み
    line(fold(&[1, 2, 3, 4, 5], 0, |a, x| a * 2 + x));
    line(fold(&[3u8, 4, 5], 1i64, |a, x| a * x as i64));

    // move と FnOnce
    let base = 100;
    let f = move || base + 1;
    line(consume(f));
    line(consume(move || base * 2));

    // impl Fn を返す
    let a5 = make_adder(5);
    line(a5(1));
    line(make_adder(100)(1));
    line(compose(make_adder(1), |x| x * 10, 4));

    // 参照で渡す / &dyn Fn
    let even = |x: i64| x % 2 == 0;
    line(if by_ref(&even, 4) { 1 } else { 0 });
    line(count_if(&[1, 2, 3, 4, 5, 6], &even));
    let lim = 3;
    line(count_if(&[1, 2, 3, 4, 5, 6], &|x| x > lim));
    let sq: &dyn Fn(i64) -> i64 = &|x| x * x;
    line(sq(9));

    // Option / Result のメソッド
    let o: Option<i64> = Some(4);
    line(o.map(|v| v * 10).unwrap_or(0));
    line(o.and_then(|v| if v > 5 { Some(v) } else { None }).unwrap_or(-1));
    let none: Option<i64> = None;
    line(none.unwrap_or_else(|| k * 7));
    line(o.map_or(0, |v| v + 1));
    line(if o.is_some_and(|v| v == 4) { 1 } else { 0 });
    line(o.filter(|v| *v > 10).unwrap_or(9));
    let r: Result<i64, u8> = Err(3);
    line(r.map(|v| v + 1).unwrap_or_else(|e| e as i64 * 100));
    line(r.map_err(|e| e as i64 + 1).unwrap_err());
    let r2: Result<i64, u8> = Ok(8);
    line(r2.and_then(|v| if v > 5 { Ok(v * 2) } else { Err(0) }).unwrap());

    // struct に入れたクロージャ
    let bonus = 1000;
    let b = Button {
        id: 7,
        on_click: move |id| id + bonus,
    };
    line(b.click());

    // 入れ子のクロージャ・メソッド呼び出しを含むキャプチャ
    let mut c = Counter { n: 0 };
    let mut step = |k: i64| c.bump_by(k);
    step(5);
    step(6);
    line(c.n);
    let outer = 3;
    let mk = |y: i64| {
        let inner = |z: i64| z * outer;
        inner(y) + 1
    };
    line(mk(4));
}
