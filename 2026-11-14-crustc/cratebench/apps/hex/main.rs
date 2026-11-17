// hex: encode / decode / ToHex / FromHex / エラー
use hex::{FromHex, ToHex};

fn main() {
    println!("{}", hex::encode("Hello world!"));
    println!("{}", hex::encode_upper(vec![1u8, 2, 0xab, 0xff]));
    println!("{:?}", hex::decode("48656c6c6f"));
    println!("{:?}", hex::decode("123"));
    println!("{:?}", hex::decode("zz"));
    let s: String = b"kiwi".encode_hex();
    println!("{s}");
    println!("{:?}", <[u8; 4]>::from_hex("01020304"));
    println!("{:?}", Vec::<u8>::from_hex("0g"));
    let mut buf = [0u8; 8];
    println!("{:?} {:?}", hex::encode_to_slice(b"\x01\x02\x03\x04", &mut buf), std::str::from_utf8(&buf));
    // (往復を何度も)
    let data: Vec<u8> = (0..4096u32).map(|i| (i * 31 + 7) as u8).collect();
    let mut sum = 0u64;
    for _ in 0..300 {
        let e = hex::encode(&data);
        let d = hex::decode(&e).unwrap();
        sum += d.iter().map(|&b| b as u64).sum::<u64>() + e.len() as u64;
    }
    println!("sum {sum}");
    println!("done");
}
