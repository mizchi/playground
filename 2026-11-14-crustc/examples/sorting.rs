// sort (マージソート) / binary_search / clamp のサンプル: 大きな Vec の sort、安定性、Drop を持つ要素、
// 配列・スライスの sort と binary_search、重複のある binary_search、clamp (整数・Ord な struct)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Ver(u32, u32);

fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *s >> 33
}

fn is_sorted(v: &[i64]) -> bool {
    let mut i = 1;
    while i < v.len() {
        if v[i - 1] > v[i] {
            return false;
        }
        i += 1;
    }
    true
}

fn main() {
    // 大きな Vec (マージが何段も走る)
    let mut seed = 42u64;
    let mut big: Vec<i64> = Vec::new();
    for _ in 0..5000 {
        big.push((lcg(&mut seed) % 1000) as i64 - 500);
    }
    big.sort();
    let s: i64 = big.iter().sum();
    println!("{} {} {} {} {}", is_sorted(&big), big[0], big[2500], big[4999], s);
    // 安定性: キーが同じなら元の順
    let mut pairs: Vec<(u32, usize)> = Vec::new();
    for i in 0..100 {
        pairs.push(((lcg(&mut seed) % 5) as u32, i));
    }
    pairs.sort_by_key(|p| p.0);
    let mut stable = true;
    let mut i = 1;
    while i < pairs.len() {
        if pairs[i - 1].0 == pairs[i].0 && pairs[i - 1].1 > pairs[i].1 {
            stable = false;
        }
        i += 1;
    }
    println!("{} {:?} {:?}", stable, pairs[0], pairs[99]);
    // Drop を持つ要素
    let mut words: Vec<String> = Vec::new();
    for i in 0..40 {
        words.push(format!("w{}", (i * 7919) % 97));
    }
    words.sort_by(|a, b| b.cmp(a));
    println!("{} {} {}", words[0], words[20], words[39]);
    words.sort_unstable();
    println!("{:?}", &words[..5]);
    // binary_search (重複あり・なし)
    let v = vec![1, 3, 3, 3, 5, 8, 13];
    println!("{:?} {:?} {:?} {:?} {:?}", v.binary_search(&5), v.binary_search(&3), v.binary_search(&4), v.binary_search(&0), v.binary_search(&99));
    let dup = vec![2, 2, 2, 2, 2, 2, 2, 2, 2];
    println!("{:?}", dup.binary_search(&2));
    let pos = big.binary_search(&big[1234]).is_ok();
    let ins = match v.binary_search(&6) {
        Ok(i) => i,
        Err(i) => i,
    };
    println!("{} {}", pos, ins);
    let r = pairs.binary_search_by(|p| p.0.cmp(&3).then(std::cmp::Ordering::Greater));
    let k = pairs.binary_search_by_key(&4, |p| p.0);
    println!("{:?} {}", r, k.is_ok());
    // 配列・スライス
    let mut arr = [9, 4, 7, 1, 8, 2];
    arr.sort();
    println!("{:?} {:?}", arr, arr.binary_search(&7));
    let mut arr2 = [(2, 'b'), (1, 'z'), (2, 'a')];
    arr2.sort_by_key(|p| p.0);
    println!("{:?}", arr2);
    let sl = &mut big[10..20];
    sl.sort_by(|a, b| b.cmp(a));
    println!("{:?} {:?}", &big[10..13], big[..10].binary_search(&-500));
    // clamp
    println!("{} {} {} {}", 15.clamp(0, 10), (-3i64).clamp(-2, 2), 5u8.clamp(1, 9), Ver(1, 9).clamp(Ver(1, 2), Ver(1, 5)).1);
    let c = String::from("m").clamp(String::from("a"), String::from("f"));
    println!("{}", c);
}
