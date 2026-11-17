// cfg-if: if / else if / else の cfg を選ぶマクロ
cfg_if::cfg_if! {
    if #[cfg(target_os = "windows")] {
        fn os() -> &'static str { "windows" }
    } else if #[cfg(all(unix, target_pointer_width = "64"))] {
        fn os() -> &'static str { "unix64" }
    } else {
        fn os() -> &'static str { "other" }
    }
}

cfg_if::cfg_if! {
    if #[cfg(debug_assertions)] {
        fn mode() -> u32 { 1 }
    } else {
        fn mode() -> u32 { 2 }
    }
}

fn main() {
    println!("{}", os());
    let mut s = 0u64;
    for i in 0..50_000_000u64 {
        s = s.wrapping_add(i * mode() as u64 % 7);
    }
    println!("{}", s > 0);
    println!("done");
}
