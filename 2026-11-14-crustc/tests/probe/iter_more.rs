fn main() {
    let v = vec![3, 1, 4, 1, 5, 9, 2, 6];
    let w: Vec<(usize, i32)> = v.iter().copied().enumerate().filter(|(i, _)| i % 2 == 0).collect();
    println!("{:?}", w);
    println!("{:?}", v.iter().zip(v.iter().skip(1)).map(|(a, b)| b - a).collect::<Vec<_>>());
    println!("{:?} {:?}", v.iter().position(|&x| x == 5), v.iter().rposition(|&x| x == 1));
    println!("{:?} {:?}", v.iter().max_by_key(|&&x| (x % 5, x)), v.iter().min_by(|a, b| b.cmp(a)));
    println!("{}", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("-"));
    let (even, odd): (Vec<i32>, Vec<i32>) = v.iter().partition(|&&x| x % 2 == 0);
    println!("{:?} {:?}", even, odd);
    println!("{:?}", v.chunks(3).map(|c| c.iter().sum::<i32>()).collect::<Vec<_>>());
    println!("{:?}", (1..=4).flat_map(|i| (0..i).map(move |j| i * 10 + j)).collect::<Vec<_>>());
    println!("{}", v.iter().rev().take_while(|&&x| x > 1).count());
    println!("{:?}", v.iter().fold(String::new(), |acc, x| acc + &x.to_string()));
    let mut s = v.clone(); s.sort_unstable(); s.dedup(); println!("{:?}", s);
    let mut r = v.clone(); r.retain(|&x| x > 2); println!("{:?}", r);
    println!("{:?}", v.iter().last());
    println!("{:?}", v.iter().cycle().skip(6).take(5).collect::<Vec<_>>());
    println!("{}", (1..=10).filter(|x| x % 3 == 0).product::<i32>());
    let names = ["b", "a", "c"];
    let mut sorted: Vec<_> = names.iter().collect(); sorted.sort(); println!("{:?}", sorted);
    println!("{:?}", v.iter().skip(2).step_by(2).collect::<Vec<_>>());
    println!("{}", v.iter().any(|&x| x > 8) && v.iter().all(|&x| x > 0));
    println!("{:?}", v.windows(2).filter(|w| w[0] < w[1]).count());
}
