//! 合成 URL データ (TS 版 `urls.ts` の移植。乱数系列は異なる)。
use crate::Rng;
use std::collections::HashSet;

const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const WORDS: &[&str] = &[
    "news", "blog", "shop", "cloud", "photo", "music", "travel", "food", "game", "tech", "sports", "book",
    "garden", "health", "movie", "code", "design", "school", "city", "market", "home", "pet", "car", "art",
    "docs", "wiki", "forum", "jobs", "weather", "maps", "video", "radio", "kitchen", "craft", "science",
];
const BRANDS: &[&str] = &["paypal", "apple", "amazon", "bank", "netflix", "google", "microsoft", "wallet", "post", "card"];
const BAIT: &[&str] = &["secure", "login", "verify", "account", "update", "support", "signin", "confirm"];
const GOOD_TLD: &[&str] = &[".com", ".org", ".net", ".jp", ".io", ".dev", ".co.uk"];
const BAD_TLD: &[&str] = &[".xyz", ".top", ".info", ".com", ".ru", ".click", ".shop", ".net"];

fn int(r: &mut Rng, lo: usize, hi: usize) -> usize {
    lo + r.below(hi - lo + 1)
}
fn token(r: &mut Rng, n: usize) -> String {
    (0..n).map(|_| *r.pick(ALNUM) as char).collect()
}

pub fn benign(r: &mut Rng) -> String {
    let www = if r.f64() < 0.5 { "www." } else { "" };
    let w1 = r.pick(WORDS);
    let w2 = if r.f64() < 0.3 { *r.pick(WORDS) } else { "" };
    let tld = r.pick(GOOD_TLD);
    let depth = int(r, 0, 3);
    let path: Vec<String> = (0..depth)
        .map(|_| if r.f64() < 0.8 { r.pick(WORDS).to_string() } else { int(r, 1, 9999).to_string() })
        .collect();
    let mut u = format!("https://{www}{w1}{w2}{tld}/{}", path.join("/"));
    if r.f64() < 0.03 {
        u += &format!("/{}", r.pick(BAIT));
    }
    if r.f64() < 0.15 {
        u += &format!("?id={}", int(r, 1, 99999));
    }
    u
}

pub fn malicious(r: &mut Rng) -> String {
    let p = r.f64();
    if p < 0.15 {
        let (w, t, w2) = (r.pick(WORDS), r.pick(GOOD_TLD), r.pick(WORDS));
        let n = int(r, 4, 8);
        return format!("https://{w}{t}/{w2}/{}", token(r, n));
    }
    let scheme = if r.f64() < 0.7 { "http" } else { "https" };
    let host = if p < 0.6 {
        let (b, sep, bait) = (r.pick(BRANDS), r.pick(&["-", ".", ""]), r.pick(BAIT));
        let num = if r.f64() < 0.5 { int(r, 1, 999).to_string() } else { String::new() };
        format!("{b}{sep}{bait}{num}{}", r.pick(BAD_TLD))
    } else {
        let n = int(r, 6, 14);
        format!("{}{}", token(r, n), r.pick(BAD_TLD))
    };
    let bait = if r.f64() < 0.5 { format!("{}/", r.pick(BAIT)) } else { String::new() };
    let n = int(r, 6, 16);
    let mut u = format!("{scheme}://{host}/{bait}{}", token(r, n));
    if r.f64() < 0.4 {
        u += r.pick(&[".php", ".html", "/index.php"]);
    }
    if r.f64() < 0.3 {
        u += &format!("?session={}", token(r, 10));
    }
    u
}

pub fn unique(r: &mut Rng, n: usize, gen: fn(&mut Rng) -> String, exclude: &HashSet<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let u = gen(r);
        if !exclude.contains(&u) && seen.insert(u.clone()) {
            out.push(u);
        }
    }
    out
}
