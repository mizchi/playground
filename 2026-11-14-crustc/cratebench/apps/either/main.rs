// either: Either<L, R> の作り方・map・either・iter・into_iter・partition
use either::{Either, Left, Right};

fn parse(s: &str) -> Either<i64, String> {
    match s.parse::<i64>() {
        Ok(n) => Left(n),
        Err(_) => Right(s.to_uppercase()),
    }
}

fn main() {
    let xs = ["12", "abc", "-7", "x1", "40"];
    for s in xs {
        let e = parse(s);
        let d = e.as_ref().map_left(|n| n * 2).map_right(|t| t.len());
        println!("{:?} {:?} {} {}", e, d, e.is_left(), e.clone().either(|n| n.to_string(), |t| t));
    }
    let v: Vec<Either<i64, String>> = xs.iter().map(|s| parse(s)).collect();
    let lefts: Vec<i64> = v.iter().filter_map(|e| e.as_ref().left().copied()).collect();
    let rights: Vec<&String> = v.iter().filter_map(|e| e.as_ref().right()).collect();
    println!("{:?} {:?}", lefts, rights);
    let it: Either<std::ops::Range<i32>, std::vec::IntoIter<i32>> = if lefts.len() > 2 { Left(0..5) } else { Right(vec![9, 8].into_iter()) };
    println!("{:?}", it.collect::<Vec<i32>>());
    let flip = Left::<i32, &str>(3).flip();
    println!("{:?} {:?}", flip, flip.unwrap_right());
    let mut total = 0i64;
    for i in 0..3_000_000i64 {
        let e: Either<i64, u32> = if i % 3 == 0 { Left(i) } else { Right(i as u32) };
        total += e.either(|a| a * 2, |b| b as i64);
    }
    println!("{total}");
    println!("done");
}
