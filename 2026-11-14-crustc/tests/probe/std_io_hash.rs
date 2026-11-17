// std の fs / env / io::Error / hash (DefaultHasher は SipHash-1-3 で値も std と同じ) / bool::then / 型の別名 / to_le_bytes
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
type Key = u128;
type Res<T> = Result<T, String>;
fn parse(s: &str) -> Res<i32> {
    s.parse::<i32>().map_err(|e| format!("bad {}: {}", s, e))
}
fn main() {
    let path = "/tmp/crustc_probe_std_io_hash.txt";
    fs::write(path, "alpha\nbeta\n").unwrap();
    let s = fs::read_to_string(path).unwrap();
    println!("{:?} {}", s, fs::read(path).unwrap().len());
    match fs::read_to_string("/nonexistent/dir/x") {
        Ok(_) => println!("?"),
        Err(e) => println!("{} | {:?}", e, e),
    }
    let r: std::io::Result<()> = Ok(());
    println!("{:?} {:?}", r, std::env::var("CRUSTC_PROBE_NOT_SET"));
    println!("{}", std::env::args().count());
    let mut h = DefaultHasher::new();
    "hello".hash(&mut h);
    42u64.hash(&mut h);
    (&[1u8, 2, 3][..]).hash(&mut h);
    let k: Key = (h.finish() as u128) << 64 | 7;
    println!("{:x}", k);
    let mut h2 = DefaultHasher::new();
    h2.write(b"abcdefghijklmnop");
    h2.write_u32(9);
    println!("{:x}", h2.finish());
    println!("{:?} {:?}", parse("12"), parse("x"));
    println!("{:?} {:?}", (1 < 2).then_some('y'), (1 > 2).then(|| 5));
    println!("{:?} {:?} {}", 0x1234u16.to_le_bytes(), 0x01020304u32.to_be_bytes(), u32::from_le_bytes([1, 0, 0, 0]));
    let w = b"pub fn";
    println!("{} {}", matches!(&w[..3], b"pub" | b"fn"), &w[4..] == b"fn");
    println!("{}", String::from_utf8_lossy(&[104, 105, 0xff, 33]));
    let _ = fs::write(path, "");
}
