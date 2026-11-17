"""crustc 自身の crates/core/src/enc.rs と crates/core/src/fxhash.rs を 1 ファイルのプログラムにまとめる。
main は enc_struct! で符号化できる struct を作って符号化・復号し、バイト列と結果を出す。FxHasher のハッシュも出す。"""
enc = open("crates/core/src/enc.rs").read()
fx = open("crates/core/src/fxhash.rs").read()
print("""// 自動生成: tests/selfhost/build_enc.py (crustc の crates/core/src/enc.rs と crates/core/src/fxhash.rs をそのまま含む)
mod enc {
""" + enc + """}

mod fxhash {
""" + fx + """}

use enc::Enc;
use std::hash::Hasher;

#[derive(Default, Debug, Clone, PartialEq)]
struct Item {
    name: String,
    n: u64,
    off: i64,
    flag: bool,
    kids: Vec<String>,
    pair: Option<(u64, String)>,
    tri: Vec<(bool, i64, String)>,
}
crate::enc_struct!(Item { name, n, off, flag, kids, pair, tri });

fn main() {
    let items = vec![
        Item::default(),
        Item { name: "main".into(), n: 300, off: -5, flag: true, kids: vec!["a".into(), "bc".into()], pair: Some((7, "x".into())), tri: vec![(true, -1, "t".into()), (false, 1 << 40, "".into())] },
        Item { name: "ünïcode".into(), n: u64::MAX, off: i64::MIN, ..Default::default() },
    ];
    for it in &items {
        let mut out = vec![];
        it.enc(&mut out);
        let mut r = enc::Rd::new(&out);
        let back = Item::dec(&mut r);
        println!("{:?} -> {} bytes {:?} ok={}", it.name, out.len(), &out[..out.len().min(24)], back.as_ref() == Some(it));
    }
    let v: Vec<Item> = items.clone();
    let mut out = vec![];
    v.enc(&mut out);
    let back: Option<Vec<Item>> = Enc::dec(&mut enc::Rd::new(&out));
    println!("vec {} bytes ok={}", out.len(), back == Some(items));
    let mut h = fxhash::FxHasher::default();
    h.write(b"hello world, fx!");
    h.write_u32(7);
    h.write_usize(99);
    println!("fx {:x}", h.finish());
    let mut m: fxhash::FxMap<String, u32> = fxhash::FxMap::default();
    for (i, w) in "a b c a b a".split(' ').enumerate() {
        *m.entry(w.to_string()).or_insert(0) += i as u32;
    }
    let mut kv: Vec<_> = m.into_iter().collect();
    kv.sort();
    println!("{:?}", kv);
}
""")
