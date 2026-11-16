// adler2: Adler-32 (一度に・分けて・Hasher として・BufRead から)
use std::hash::Hasher;

fn main() {
    println!("{:08x}", adler2::adler32_slice(b"Wikipedia"));
    let mut a = adler2::Adler32::new();
    a.write_slice(b"Wiki");
    a.write_slice(b"pedia");
    println!("{:08x}", a.checksum());
    let mut h = adler2::Adler32::from_checksum(1);
    h.write(b"hello");
    println!("{:x}", h.finish());
    println!("{:08x}", adler2::adler32(&b"from a reader"[..]).unwrap());
    // (1 MiB を 64 回)
    let data: Vec<u8> = (0..1 << 20).map(|i: u32| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let mut acc = 0u32;
    for r in 0..64 {
        let mut a = adler2::Adler32::from_checksum(r);
        a.write_slice(&data[r as usize..]);
        acc ^= a.checksum();
    }
    println!("acc {acc:08x}");
    println!("done");
}
