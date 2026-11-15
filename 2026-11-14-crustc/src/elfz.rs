//! `.o` の機械語 (`.text`) を ELF 標準の圧縮セクション (SHF_COMPRESSED, zstd) にする。
//!
//! wild はどのセクションでも SHF_COMPRESSED を解凍して読むので、圧縮した `.o` をそのまま渡せる。
//! (gABI では SHF_ALLOC のセクションの圧縮は想定外で、ld.lld / ld.bfd は読めない。
//! そちらには `uncompress` で戻したものを一時ファイルで渡す)

const SHF_COMPRESSED: u64 = 0x800;
const SHF_ALLOC: u64 = 0x2;
const SHT_PROGBITS: u32 = 1;
const SHT_NOBITS: u32 = 8;
const ELFCOMPRESS_ZSTD: u32 = 2;

fn rd(d: &[u8], o: usize, n: usize) -> Option<u64> {
    let b = d.get(o..o + n)?;
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(b);
    Some(u64::from_le_bytes(v))
}

fn wr(d: &mut [u8], o: usize, n: usize, v: u64) {
    d[o..o + n].copy_from_slice(&v.to_le_bytes()[..n]);
}

/// ELF64 の section header (必要な分)
#[derive(Clone)]
pub struct Shdr {
    pub ty: u32,
    pub flags: u64,
    pub off: usize,
    pub size: usize,
    pub align: u64,
    pub entsize: u64,
}

/// (section header の表の位置, 数, 1 つの大きさ)
fn sh_table(d: &[u8]) -> Option<(usize, usize, usize)> {
    if d.get(..4)? != b"\x7fELF" || d[4] != 2 || d[5] != 1 {
        return None;
    }
    let shoff = rd(d, 0x28, 8)? as usize;
    let shentsize = rd(d, 0x3A, 2)? as usize;
    let mut shnum = rd(d, 0x3C, 2)? as usize;
    if shnum == 0 {
        shnum = rd(d, shoff + 0x20, 8)? as usize;
    }
    Some((shoff, shnum, shentsize))
}

pub fn sections(d: &[u8]) -> Option<Vec<Shdr>> {
    let (shoff, shnum, es) = sh_table(d)?;
    (0..shnum)
        .map(|i| {
            let h = shoff + i * es;
            Some(Shdr {
                ty: rd(d, h + 4, 4)? as u32,
                flags: rd(d, h + 8, 8)?,
                off: rd(d, h + 0x18, 8)? as usize,
                size: rd(d, h + 0x20, 8)? as usize,
                align: rd(d, h + 0x30, 8)?,
                entsize: rd(d, h + 0x38, 8)?,
            })
        })
        .collect()
}

/// セクションの中身 (圧縮されていれば解凍したもの)
pub fn section_bytes<'a>(d: &'a [u8], s: &Shdr) -> Option<std::borrow::Cow<'a, [u8]>> {
    let raw = d.get(s.off..s.off + s.size)?;
    if s.flags & SHF_COMPRESSED == 0 {
        return Some(std::borrow::Cow::Borrowed(raw));
    }
    if rd(raw, 0, 4)? as u32 != ELFCOMPRESS_ZSTD {
        return None;
    }
    let n = rd(raw, 8, 8)? as usize;
    let out = zstd::bulk::decompress(raw.get(24..)?, n).ok()?;
    (out.len() == n).then_some(std::borrow::Cow::Owned(out))
}

/// セクションの中身を差し替えた ELF を作り直す (中身を詰めて並べ直し、section header の表を最後に置く)。
/// f は各セクションについて (新しい中身, 新しい flags, 新しい align) を返す (None ならそのまま)
fn rebuild(d: &[u8], f: impl Fn(usize, &Shdr) -> Option<(Vec<u8>, u64, u64)>) -> Option<Vec<u8>> {
    let (shoff, shnum, es) = sh_table(d)?;
    let secs = sections(d)?;
    let mut out = d.get(..64)?.to_vec();
    let mut hdrs = d.get(shoff..shoff + shnum * es)?.to_vec();
    for (i, s) in secs.iter().enumerate() {
        if i == 0 {
            continue;
        }
        let (data, flags, align) = match f(i, s) {
            Some(x) => x,
            None if s.ty == SHT_NOBITS => {
                continue;
            }
            None => (d.get(s.off..s.off + s.size)?.to_vec(), s.flags, s.align),
        };
        let al = align.max(1) as usize;
        while out.len() % al != 0 {
            out.push(0);
        }
        let h = i * es;
        wr(&mut hdrs, h + 8, 8, flags);
        wr(&mut hdrs, h + 0x18, 8, out.len() as u64);
        wr(&mut hdrs, h + 0x20, 8, data.len() as u64);
        wr(&mut hdrs, h + 0x30, 8, align);
        out.extend_from_slice(&data);
    }
    while out.len() % 8 != 0 {
        out.push(0);
    }
    let new_shoff = out.len() as u64;
    out.extend_from_slice(&hdrs);
    wr(&mut out, 0x28, 8, new_shoff);
    Some(out)
}

/// メモリに載る PROGBITS (`.text` / `.rodata` / `.data.rel.ro` など) を zstd で圧縮する。圧縮で小さくならなければ元のまま
pub fn compress_text(d: &[u8]) -> Vec<u8> {
    let r = rebuild(d, |_, s| {
        // (機械語と、読み出し専用のデータ (panic の表・文字列)。どちらも wild はそのまま読む)
        if s.ty != SHT_PROGBITS || s.flags & SHF_ALLOC == 0 || s.flags & SHF_COMPRESSED != 0 || s.size < 256 {
            return None;
        }
        let raw = d.get(s.off..s.off + s.size)?;
        let z = zstd::bulk::compress(raw, 1).ok()?;
        if z.len() + 24 >= raw.len() {
            return None;
        }
        // Elf64_Chdr: ch_type, ch_reserved, ch_size, ch_addralign
        let mut v = Vec::with_capacity(24 + z.len());
        v.extend_from_slice(&ELFCOMPRESS_ZSTD.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&(raw.len() as u64).to_le_bytes());
        v.extend_from_slice(&s.align.max(1).to_le_bytes());
        v.extend_from_slice(&z);
        Some((v, s.flags | SHF_COMPRESSED, 8))
    });
    r.unwrap_or_else(|| d.to_vec())
}

/// 圧縮したセクションを元に戻す (ld.lld / ld.bfd に渡すため)
pub fn uncompress(d: &[u8]) -> Option<Vec<u8>> {
    rebuild(d, |_, s| {
        if s.flags & SHF_COMPRESSED == 0 {
            return None;
        }
        let raw = d.get(s.off..s.off + s.size)?;
        let align = rd(raw, 16, 8)?;
        let data = section_bytes(d, s)?.into_owned();
        Some((data, s.flags & !SHF_COMPRESSED, align))
    })
}

/// 定義された関数のシンボルの (名前, セクション番号, 位置)
pub fn func_symbols(d: &[u8]) -> Option<Vec<(String, usize, u64)>> {
    let (shoff, _, es) = sh_table(d)?;
    let secs = sections(d)?;
    let mut out = vec![];
    for (i, s) in secs.iter().enumerate() {
        if s.ty != 2 || s.entsize != 24 {
            continue;
        }
        let link = rd(d, shoff + i * es + 0x28, 4)? as usize;
        let st = secs.get(link)?;
        for k in 0..s.size / 24 {
            let e = s.off + k * 24;
            let (name, info, shndx) = (rd(d, e, 4)? as usize, *d.get(e + 4)?, rd(d, e + 6, 2)? as usize);
            if info & 0xF != 2 || shndx == 0 {
                continue;
            }
            let ns = st.off + name;
            let end = ns + d.get(ns..)?.iter().position(|&c| c == 0)?;
            out.push((String::from_utf8_lossy(&d[ns..end]).into_owned(), shndx, rd(d, e + 8, 8)?));
        }
    }
    Some(out)
}

/// .o に圧縮したセクションがあるか
pub fn is_compressed(d: &[u8]) -> bool {
    sections(d).is_some_and(|ss| ss.iter().any(|s| s.flags & SHF_COMPRESSED != 0))
}
