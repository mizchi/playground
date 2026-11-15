fn lcs(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() { for j in 1..=b.len() {
        dp[i][j] = if a[i - 1] == b[j - 1] { dp[i - 1][j - 1] + 1 } else { dp[i - 1][j].max(dp[i][j - 1]) };
    } }
    dp[a.len()][b.len()]
}
fn sieve(n: usize) -> Vec<usize> {
    let mut is = vec![true; n + 1]; is[0] = false; if n >= 1 { is[1] = false; }
    let mut i = 2;
    while i * i <= n { if is[i] { let mut j = i * i; while j <= n { is[j] = false; j += i; } } i += 1; }
    is.iter().enumerate().filter(|&(_, &p)| p).map(|(i, _)| i).collect()
}
fn fib_memo(n: u64, memo: &mut std::collections::HashMap<u64, u64>) -> u64 {
    if n < 2 { return n; }
    if let Some(&v) = memo.get(&n) { return v; }
    let v = fib_memo(n - 1, memo) + fib_memo(n - 2, memo);
    memo.insert(n, v);
    v
}
fn quicksort<T: PartialOrd + Copy>(v: &mut [T]) {
    if v.len() <= 1 { return; }
    let last = v.len() - 1;
    let p = v[last];
    let mut i = 0;
    for j in 0..last { if v[j] < p { v.swap(i, j); i += 1; } }
    v.swap(i, last);
    let (l, r) = v.split_at_mut(i);
    quicksort(l); quicksort(&mut r[1..]);
}
fn main() {
    println!("{}", lcs("ABCBDAB", "BDCABA"));
    println!("{:?}", sieve(50));
    let mut m = std::collections::HashMap::new();
    println!("{}", fib_memo(80, &mut m));
    let mut v = vec![9, 3, 7, 1, 8, 2, 5];
    quicksort(&mut v);
    println!("{:?}", v);
    let coins = [1, 5, 10, 25];
    let mut ways = vec![0u64; 101]; ways[0] = 1;
    for &c in &coins { for x in c..=100 { ways[x] += ways[x - c]; } }
    println!("{}", ways[100]);
}
