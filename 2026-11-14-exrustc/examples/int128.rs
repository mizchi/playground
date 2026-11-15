// 128 bit 整数: 算術・除算・書式化・キャスト・メソッド・コレクション
use std::collections::HashMap;

fn rng(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

fn fib(n: u32) -> u128 {
    let (mut a, mut b) = (0u128, 1u128);
    for _ in 0..n {
        let t = a + b;
        a = b;
        b = t;
    }
    a
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Big {
    v: i128,
    tag: u8,
}

fn main() {
    println!("{} {}", u128::MAX, i128::MIN);
    println!("fib(180) = {}", fib(180));
    println!("{:#x} {:o} {:b}", fib(150), 1u128 << 100, 255i128);
    println!("{:>50}|{:<10}|{:+}|{:08}", fib(170), 7u128, 42i128, -5i128);
    let n: i128 = -123456789012345678901234567890;
    println!("{} {} {} {}", n / 7, n % 7, n / -1000000007, n % -1000000007);
    println!("{} {}", n.abs(), n.signum());
    println!("{} {} {}", 3u128.pow(80), 2i128.pow(126), (-3i128).pow(77));
    let x: u128 = 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210;
    println!("{} {} {}", x.count_ones(), x.leading_zeros(), x.trailing_zeros());
    println!("{:?} {:?}", u128::MAX.checked_add(1), 5u128.checked_sub(3));
    println!("{} {}", u128::MAX.wrapping_add(2), i128::MAX.wrapping_add(1));
    println!("{:?} {:?}", 10u128.checked_div(0), 10i128.checked_rem(3));
    println!("{} {}", x.min(5), x.max(5));
    println!("{} {} {}", x as u8, x as i64, (n as u64) as u128);
    println!("{} {} {}", n as f64, x as f32, (1e30f64) as u128);
    println!("{} {} {}", (-1e40f64) as i128, f64::NAN as u128, 1e39f64 as u128);
    let p: u128 = "340282366920938463463374607431768211455".parse().unwrap();
    let q: i128 = "-170141183460469231731687303715884105728".parse().unwrap();
    println!("{} {} {:?}", p, q, "340282366920938463463374607431768211456".parse::<u128>().is_err());
    println!("{}", (p / 3).to_string().len());

    let mut m: HashMap<u128, Big> = HashMap::new();
    let mut v = vec![];
    let mut s = 0x9e3779b97f4a7c15u64;
    for i in 0..20u8 {
        let a = ((rng(&mut s) as u128) << 64 | rng(&mut s) as u128) as i128;
        let b = Big { v: a >> (i as u32 * 5), tag: i };
        m.insert(a as u128, b);
        v.push(b);
    }
    v.sort();
    println!("{:?}", &v[..3]);
    println!("{} {:?}", m.len(), v.iter().map(|b| b.v).max());
    let total: i128 = v.iter().map(|b| b.v / 1000).sum();
    println!("{}", total);

    // 乱数で四則演算・シフト・比較を照合する
    let mut h: u64 = 0;
    for _ in 0..3000 {
        let a = (rng(&mut s) as u128) << (rng(&mut s) % 64) | rng(&mut s) as u128;
        let b = (rng(&mut s) as u128) >> (rng(&mut s) % 64) | 1;
        let sa = a as i128;
        let sb = (b as i128) * if rng(&mut s) % 2 == 0 { 1 } else { -1 };
        let r = [
            a / b,
            a % b,
            (sa / sb) as u128,
            (sa % sb) as u128,
            a.wrapping_mul(b),
            a ^ (b << 3),
            (sa >> 7) as u128,
            (a < b) as u128 + ((sa < sb) as u128) * 2,
        ];
        for t in r {
            h = h.wrapping_mul(1000003) ^ (t as u64) ^ ((t >> 64) as u64);
        }
        if rng(&mut s) % 1000 == 0 {
            println!("{} {} {} {}", a, sb, a / b, sa % sb);
        }
    }
    println!("hash {}", h);
}
