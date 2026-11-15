// BinaryHeap と sort_by_cached_key のサンプル: push / pop / peek、Reverse で最小ヒープ、from / collect、
// iter / into_vec / into_sorted_vec (並びも std と同じ)、{:?}、String とタプルの要素、ダイクストラ法、
// sort_by_cached_key (キーの計算は 1 要素 1 回)
use std::cmp::Reverse;
use std::collections::BinaryHeap;

fn dijkstra(adj: &Vec<Vec<(usize, u64)>>, s: usize) -> Vec<u64> {
    let mut dist = vec![u64::MAX; adj.len()];
    let mut h = BinaryHeap::new();
    dist[s] = 0;
    h.push(Reverse((0u64, s)));
    while let Some(Reverse((d, u))) = h.pop() {
        if d > dist[u] {
            continue;
        }
        for &(v, w) in &adj[u] {
            let nd = d + w;
            if nd < dist[v] {
                dist[v] = nd;
                h.push(Reverse((nd, v)));
            }
        }
    }
    dist
}

fn main() {
    let mut h = BinaryHeap::new();
    for x in [5, 1, 8, 3, 9, 2, 8, 7] {
        h.push(x);
    }
    println!("{} {:?} {:?}", h.len(), h.peek(), h);
    let v: Vec<i32> = h.iter().copied().collect();
    println!("{:?}", v);
    let mut out = vec![];
    while let Some(x) = h.pop() {
        out.push(x);
    }
    println!("{:?} {} {:?}", out, h.is_empty(), h.pop());
    // 最小ヒープ
    let mut mh = BinaryHeap::new();
    for x in [5u32, 1, 8, 3] {
        mh.push(Reverse(x));
    }
    let Reverse(m) = mh.pop().unwrap();
    println!("{} {:?}", m, mh.peek());
    // from / collect / into_vec / into_sorted_vec
    let fh = BinaryHeap::from(vec![3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5]);
    println!("{:?}", fh.into_vec());
    let ch: BinaryHeap<i64> = (1..=10).map(|x| (x * 37) % 11).collect();
    println!("{:?}", ch.into_sorted_vec());
    // String / タプル
    let mut sh: BinaryHeap<String> = BinaryHeap::new();
    for w in ["pear", "apple", "zoo", "fig"] {
        sh.push(w.to_string());
    }
    println!("{:?} {:?}", sh.pop(), sh.into_sorted_vec());
    let mut th = BinaryHeap::new();
    th.push((2, "b"));
    th.push((9, "a"));
    th.push((2, "z"));
    println!("{:?} {}", th.pop(), th.len());
    let mut total = 0;
    for x in th.into_iter() {
        total += x.0;
    }
    println!("{}", total);
    // ダイクストラ法
    let adj: Vec<Vec<(usize, u64)>> = vec![
        vec![(1, 4), (2, 1)],
        vec![(3, 1)],
        vec![(1, 2), (3, 5)],
        vec![(4, 3)],
        vec![],
    ];
    println!("{:?}", dijkstra(&adj, 0));
    // sort_by_cached_key
    let mut calls = 0;
    let mut words = vec!["banana", "Apple", "cherry", "apple", "Banana", "date"];
    words.sort_by_cached_key(|w| {
        calls += 1;
        lower(w)
    });
    println!("{:?} {}", words, calls);
    let mut nums = vec![-5i32, 3, -1, 4, -3, 2, 0];
    nums.sort_by_cached_key(|x| x.abs());
    println!("{:?}", nums);
    let mut arr = [(1, 'c'), (0, 'a'), (1, 'b')];
    arr.sort_by_cached_key(|p| p.0);
    println!("{:?}", arr);
}

fn lower(w: &str) -> String {
    let mut s = String::new();
    for c in w.chars() {
        s.push(if c >= 'A' && c <= 'Z' { ((c as u8) + 32) as char } else { c });
    }
    s
}
