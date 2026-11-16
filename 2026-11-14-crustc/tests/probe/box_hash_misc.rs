// Box からのムーブ・Vec<enum> のハッシュ・u128 の掛け算の検査・MutexGuard の添字・zip / chain の配列・Range::contains など
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum K {
    A,
    P(u8),
    S(u32),
}

#[derive(Clone, Debug)]
enum T {
    N(String),
    A(Box<T>, u32),
}

mod inner {
    pub static LIMIT: u32 = 40;
}

fn h<X: Hash + ?Sized>(v: &X) -> u64 {
    let mut s = DefaultHasher::new();
    v.hash(&mut s);
    s.finish()
}

fn first(v: &mut Vec<Box<String>>) -> &mut String {
    &mut v[0]
}

fn main() {
    // Box の中身のムーブ
    let mut todo = vec![T::A(Box::new(T::A(Box::new(T::N("x".into())), 2)), 1), T::N("y".into())];
    let mut out = vec![];
    while let Some(t) = todo.pop() {
        match t {
            T::N(n) => out.push(n),
            T::A(e, _) => todo.push(*e),
        }
    }
    println!("{out:?}");
    // Vec<enum> / タプルのキー
    let mut m: HashMap<(u32, Vec<K>), u32> = HashMap::new();
    for i in 0..20u32 {
        m.insert((i % 5, vec![K::P(i as u8), K::S(i), K::A]), i);
    }
    println!("{} {:?} {:?}", m.len(), m.get(&(3, vec![K::P(13), K::S(13), K::A])), m.get(&(0, vec![])));
    // std と同じハッシュ値
    let o: Option<String> = Some("x".into());
    let none: Option<u8> = None;
    println!("{:x} {:x} {:x}", h(&("tuple", 3usize)), h(&(100u8, true, &o)), h(&none));
    // 128 bit の掛け算の検査
    let big = u64::MAX as u128;
    println!("{:?} {:?} {:?}", big.checked_mul(big), (big + 1).checked_mul(big << 64), i128::MIN.checked_mul(-1));
    println!("{:?}", (-(1i128 << 63)).overflowing_mul(1i128 << 64));
    // MutexGuard の deref・添字
    let mx = Mutex::new(vec![Box::new(1u32), Box::new(2)]);
    {
        let mut g = mx.lock().unwrap();
        *g[1] = 5;
        (*g)[0] = Box::new(7);
    }
    let g = mx.lock().unwrap();
    println!("{} {} {}", *g[0], *g[1], g.len());
    // &mut v[i] (Box) を &mut T に
    let mut bs = vec![Box::new("a".to_string())];
    first(&mut bs).push('b');
    println!("{}", bs[0]);
    // zip / chain に配列・スライス
    let a = vec![1u32, 2, 3];
    let r = 9u32;
    let n = a.iter().zip(&a).chain([(&r, &r)]).filter(|(x, y)| x == y).count();
    let w: Vec<u32> = a.iter().copied().chain([7u32, 8]).collect();
    println!("{n} {w:?}");
    // Range::contains・next_multiple_of・モジュールの static・&T → *const T
    let ok: Vec<bool> = [0i64, 255, 256, -1].iter().map(|&d| (0..=255).contains(&d)).collect();
    println!("{ok:?} {} {} {}", (0..5).contains(&3), 13u32.next_multiple_of(8), 16u64.next_multiple_of(8));
    let p: *const u32 = &inner::LIMIT;
    println!("{}", unsafe { *p } + 2);
}
