// 値として使うタプルのバリアント / タプル struct・`None::<T>`・ユーザの Index・キャプチャごとの捕まえ方・as_slice
#[derive(Debug, Clone)]
enum T {
    S(u32),
    P(u8, String),
    N,
}

#[derive(Debug)]
struct W(i64);

#[derive(Debug)]
enum G<X> {
    One(X),
    Zero,
}

struct Tab {
    v: Vec<String>,
}

impl std::ops::Index<usize> for Tab {
    type Output = String;
    fn index(&self, i: usize) -> &String {
        &self.v[i]
    }
}

fn pair(t: (u32, W)) -> i64 {
    t.1 .0 + t.0 as i64
}

fn main() {
    let a: Option<u32> = Some(3);
    println!("{:?} {:?}", a.map(T::S), None::<u32>.map(T::S).unwrap_or(T::N));
    let v: Vec<T> = vec![1u32, 2].into_iter().map(T::S).collect();
    let w: Vec<Option<u32>> = vec![1u32].into_iter().map(Some).collect();
    let r: Result<u32, String> = Err(5u32).map_err(|e: u32| e.to_string());
    let ws: Vec<W> = vec![4i64, 5].into_iter().map(W).collect();
    let gs: Vec<G<&str>> = vec!["x"].into_iter().map(G::One).collect();
    let f = T::P;
    println!("{v:?} {w:?} {r:?} {ws:?} {gs:?} {:?} {:?} {}", f(1, "s".into()), G::<u8>::Zero, pair((1, W(2))));
    // ユーザの Index の結果の型 (`tab[i].len()`)
    let tab = Tab { v: vec!["ab".to_string(), "cde".to_string()] };
    let name = tab[1].clone();
    println!("{} {}", tab[0].len(), matches!(name.as_str(), "cde" | "x"));
    // Copy のキャプチャを値で渡しても、Vec のキャプチャは参照のまま
    let toks: Vec<(u8, usize)> = vec![(0, 1), (1, 2), (1, 3)];
    let is_one = |k: usize| k < toks.len() && toks[k].0 == 1;
    let hits: Vec<usize> = (0..4usize).filter(|&p| is_one(p) && toks[p - 1].1 > 0).collect();
    println!("{hits:?} {}", toks.len());
    // 配列の as_slice
    let arr = [1u8, 2, 3];
    println!("{} {}", arr.as_slice().len(), b"use".as_slice() == b"use".as_slice());
}
