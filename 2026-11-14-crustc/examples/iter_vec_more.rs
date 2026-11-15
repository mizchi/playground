// イテレータと Vec のメソッドの追加分: rev (Vec / 範囲 / chars / map の後) / step_by / peekable / skip_while /
// scan / inspect / flat_map / flatten、windows / chunks、retain / dedup / drain / split_off / resize / fill /
// to_vec / join / concat / first_mut、Option / Result の expect / zip / map_or_else / get_or_insert_with など
fn main() {
    let v = vec![1, 2, 3, 4, 5, 6];
    let r: Vec<i32> = v.iter().rev().map(|x| x * 10).collect();
    let back: String = "héllo".chars().rev().collect();
    let down: Vec<u32> = (1..=5).rev().collect();
    let odd: Vec<i32> = v.iter().copied().filter(|x| x % 2 == 1).rev().collect();
    println!("{:?} {} {:?} {:?}", r, back, down, odd);
    let st: Vec<i32> = (0..20).step_by(7).collect();
    let st2: Vec<&i32> = v.iter().step_by(2).collect();
    println!("{:?} {:?}", st, st2);
    let mut p = v.iter().peekable();
    let mut runs = vec![];
    while let Some(&x) = p.next() {
        if let Some(&&n) = p.peek() {
            runs.push(x * 100 + n);
        }
    }
    println!("{:?}", runs);
    let mut digits = "123abc".chars().peekable();
    let mut num = 0;
    while let Some(d) = digits.next_if(|c| c.is_ascii_digit()) {
        num = num * 10 + d.to_digit(10).unwrap();
    }
    println!("{} {:?}", num, digits.peek());
    let sw: Vec<i32> = v.iter().copied().skip_while(|x| *x < 4).collect();
    let pre: Vec<i32> = v.iter().scan(0, |acc, x| {
        *acc += x;
        Some(*acc)
    }).collect();
    let mut seen = 0;
    let total: i32 = v.iter().inspect(|_| seen += 1).sum();
    println!("{:?} {:?} {} {}", sw, pre, total, seen);
    let fm: Vec<i32> = v.iter().take(3).flat_map(|x| vec![*x, x * 10]).collect();
    let pairs: Vec<i32> = (0..4).flat_map(|i| 0..i).collect();
    let nested = vec![vec![1, 2], vec![], vec![3]];
    let flat: Vec<i32> = nested.into_iter().flatten().collect();
    let opts = vec![Some(1), None, Some(3)];
    let somes: Vec<i32> = opts.iter().copied().flatten().collect();
    println!("{:?} {:?} {:?} {:?}", fm, pairs, flat, somes);
    for w in v.windows(3) {
        print!("{} ", w.iter().sum::<i32>());
    }
    println!();
    for c in v.chunks(4) {
        print!("{:?} ", c);
    }
    println!();
    let arr = [5, 6, 7];
    println!("{} {:?}", arr.windows(2).count(), arr.chunks(2).map(|c| c.len()).collect::<Vec<_>>());
    // Vec の編集
    let mut w = vec![3, 1, 1, 2, 2, 2, 5, 3, 3];
    w.dedup();
    println!("{:?}", w);
    w.retain(|x| *x != 2);
    println!("{:?}", w);
    let mut d: Vec<i32> = (1..=10).collect();
    let drained: Vec<i32> = d.drain(2..5).collect();
    let tail = d.split_off(5);
    println!("{:?} {:?} {:?}", drained, d, tail);
    d.drain(..2);
    d.resize(5, 0);
    println!("{:?}", d);
    d.fill(7);
    if let Some(f) = d.first_mut() {
        *f = 1;
    }
    if let Some(l) = d.last_mut() {
        *l += 1;
    }
    let copy = d[1..3].to_vec();
    println!("{:?} {:?}", d, copy);
    let mut names = vec![String::from("bob"), String::from("al"), String::from("cy")];
    names.retain(|n| n.len() > 2 || n.starts_with('a'));
    println!("{} | {} | {}", names.join(", "), ["x", "y", "z"].concat(), vec!["a", "b"].join("/"));
    let mut by_key = vec![10, 11, 20, 21, 22, 30];
    by_key.dedup_by_key(|x| *x / 10);
    println!("{:?}", by_key);
    // Option / Result
    let a: Option<i32> = Some(4);
    let b: Option<&str> = None;
    println!("{:?} {:?} {}", a.zip(Some('x')), a.zip(b), a.map_or_else(|| -1, |x| x * 2));
    let mut cache: Option<Vec<i32>> = None;
    cache.get_or_insert_with(Vec::new).push(5);
    cache.get_or_insert_with(Vec::new).push(6);
    println!("{:?} {} {:?}", cache, a.expect("present"), a.or_else(|| Some(0)).and(Some("y")));
    let ok: Result<i32, String> = Ok(3);
    let bad: Result<i32, String> = Err(String::from("no"));
    println!("{} {} {} {}", ok.clone().is_ok_and(|x| x > 2), bad.clone().is_err_and(|e| e.len() == 2), ok.clone().map_or(0, |x| x + 1), bad.map_or(0, |x| x + 1));
    let e2: Result<i32, i32> = Err(4);
    println!("{:?}", e2.or_else(|e| if e > 3 { Ok::<i32, i32>(e * 2) } else { Err(e) }));
}
