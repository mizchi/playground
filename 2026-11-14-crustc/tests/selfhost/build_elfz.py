"""crustc 自身の crates/legacy/src/elfz.rs (.o の機械語のセクションを SHF_COMPRESSED にする) を 1 ファイルのプログラムにまとめる。
zstd クレートは使えないので、同じ API の小さな代わり (0 の並びだけを縮める RLE) を `mod zstd` として付ける
(rustc 版も同じ代わりを使うので、出力は比べられる)。
usage: python3 tests/selfhost/build_elfz.py > out/selfhost/elfz_main.rs
main は引数の .o ごとに、セクション・圧縮後の大きさ・解凍して元に戻るか・関数のシンボルを出す。"""

elfz = open("crates/legacy/src/elfz.rs").read()
print("""// 自動生成: tests/selfhost/build_elfz.py (crustc の crates/legacy/src/elfz.rs をそのまま含む)
mod zstd {
    pub mod bulk {
        // 0 の並び (4 以上) を [0, 長さ (varint)] に、それ以外の 0 は [0, 0] にする
        pub fn compress(d: &[u8], _level: i32) -> std::io::Result<Vec<u8>> {
            let mut out = Vec::new();
            let mut i = 0;
            while i < d.len() {
                if d[i] != 0 {
                    out.push(d[i]);
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < d.len() && d[j] == 0 {
                    j += 1;
                }
                let mut n = (j - i) as u64;
                out.push(0);
                loop {
                    let b = (n & 0x7f) as u8;
                    n >>= 7;
                    if n == 0 {
                        out.push(b);
                        break;
                    }
                    out.push(b | 0x80);
                }
                i = j;
            }
            Ok(out)
        }
        pub fn decompress(d: &[u8], cap: usize) -> std::io::Result<Vec<u8>> {
            let mut out = Vec::with_capacity(cap);
            let mut i = 0;
            while i < d.len() {
                if d[i] != 0 {
                    out.push(d[i]);
                    i += 1;
                    continue;
                }
                i += 1;
                let mut n: u64 = 0;
                let mut sh = 0;
                loop {
                    let b = d[i];
                    i += 1;
                    n |= ((b & 0x7f) as u64) << sh;
                    sh += 7;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                for _ in 0..n {
                    out.push(0);
                }
            }
            out.truncate(cap);
            Ok(out)
        }
    }
}

mod elfz {
""" + elfz + """
use crate::zstd;
}

fn fnv(s: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in s {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let d = std::fs::read(path).unwrap();
        let Some(ss) = elfz::sections(&d) else {
            println!("{path}: not ELF");
            continue;
        };
        println!("{path}: {} bytes, {} sections, compressed={}", d.len(), ss.len(), elfz::is_compressed(&d));
        for (i, s) in ss.iter().enumerate() {
            let b = elfz::section_bytes(&d, s).map(|b| fnv(&b.to_vec()));
            println!("  [{i}] ty={} flags={:#x} off={} size={} align={} entsize={} {:x?}", s.ty, s.flags, s.off, s.size, s.align, s.entsize, b);
        }
        let z = elfz::compress_text(&d);
        println!("  compressed: {} bytes, is_compressed={}", z.len(), elfz::is_compressed(&z));
        let zs = elfz::sections(&z).unwrap();
        let same = ss.len() == zs.len()
            && ss.iter().zip(&zs).filter(|(a, _)| a.ty != 8).all(|(a, b)| elfz::section_bytes(&d, a).map(|x| x.to_vec()) == elfz::section_bytes(&z, b).map(|x| x.to_vec()));
        println!("  section contents preserved: {same}");
        match elfz::uncompress(&z) {
            Some(u) => println!("  uncompressed: {} bytes, sections={}, compressed={}", u.len(), elfz::sections(&u).map(|v| v.len()).unwrap_or(0), elfz::is_compressed(&u)),
            None => println!("  uncompress failed"),
        }
        let mut fs = elfz::func_symbols(&d).unwrap_or_default();
        fs.sort();
        println!("  {} functions: {:?}", fs.len(), &fs[..fs.len().min(6)]);
        println!("  same functions after compression: {}", elfz::func_symbols(&z).map(|mut v| { v.sort(); v }) == Some(fs));
    }
}
""")
