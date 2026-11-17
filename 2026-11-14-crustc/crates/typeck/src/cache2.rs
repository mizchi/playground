//! driver2 (syn・Cranelift を使わないドライバ) のキャッシュ。cache.rs と同じ考え方で、外部クレート (zstd) を使わない:
//! - キー: 128 bit のハッシュ (下位 64 bit をファイルに書く)
//! - 値: アイテムの要約、または関数の機械語 + シンボリックな再配置 (acg::Blob。呼び先のインスタンスは (元の関数名, 型引数) で書く)
//! - 保存するときは今回のビルドで使ったエントリだけを書く (古いものは自然に消える)
//!
//! Blob の直列化は fork した子プロセスからコンパイル結果を受け取るのにも使う (driver2 の並列化)。

use std::sync::Mutex;

use crate::acg::{Blob, BlobReloc, Target};
use crate::fxhash::FxMap;
use crate::program::{Insts, Program};

pub use crate::key::{Key, KeyHasher};

const MAGIC: &[u8; 8] = b"CRCD0001";

unsafe extern "C" {
    #[link_name = "rename"]
    fn c_rename(old: *const u8, new: *const u8) -> i32;
}

/// キャッシュファイル: MAGIC, エントリの数 (可変長), (キーの下位 64 bit, 長さ (可変長), 中身) *
pub struct Cache2 {
    data: Vec<u8>,
    index: FxMap<u64, (usize, usize)>,
}

pub struct Rd<'a> {
    pub d: &'a [u8],
    pub pos: usize,
}

impl Rd<'_> {
    pub fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(b[0] as u16 | (b[1] as u16) << 8)
    }
    pub fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        let mut v = 0u64;
        for i in 0..8 {
            v |= (b[i] as u64) << (8 * i);
        }
        Some(v)
    }
    pub fn uv(&mut self) -> Option<u64> {
        let mut v = 0u64;
        let mut sh = 0;
        loop {
            let b = self.u8()?;
            v |= ((b & 0x7F) as u64) << sh;
            if b < 0x80 {
                return Some(v);
            }
            sh += 7;
            if sh > 63 {
                return None;
            }
        }
    }
    pub fn sv(&mut self) -> Option<i64> {
        let u = self.uv()?;
        Some((u >> 1) as i64 ^ -((u & 1) as i64))
    }
}

/// 符号なしの可変長整数 (7 bit ずつ、上位ビットが続きの印)
pub fn put_uv(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// 符号つき (zigzag) の可変長整数
pub fn put_sv(out: &mut Vec<u8>, v: i64) {
    put_uv(out, ((v << 1) ^ (v >> 63)) as u64);
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_uv(out, b.len() as u64);
    out.extend_from_slice(b);
}

impl Cache2 {
    pub fn empty() -> Cache2 {
        Cache2 { data: vec![], index: FxMap::default() }
    }

    /// ファイルを読む (読めない・形式が違うなら空)
    pub fn open(path: &std::path::Path) -> Cache2 {
        let data = std::fs::read(path).unwrap_or_default();
        let mut c = Cache2::empty();
        if data.len() < 8 || &data[..8] != MAGIC {
            return c;
        }
        let mut r = Rd { d: &data, pos: 8 };
        let Some(n) = r.uv() else { return c };
        let mut idx = FxMap::default();
        for _ in 0..n {
            let (Some(k), Some(len)) = (r.u64(), r.uv()) else { return c };
            let start = r.pos;
            if r.take(len as usize).is_none() {
                return c;
            }
            idx.insert(k, (start, start + len as usize));
        }
        c.index = idx;
        c.data = data;
        c
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn get(&self, key: Key) -> Option<&[u8]> {
        let &(s, e) = self.index.get(&(key as u64))?;
        Some(&self.data[s..e])
    }

    /// エントリを書く (一時ファイルに書いて rename)。書いたバイト数
    pub fn save(path: &std::path::Path, entries: &[(Key, &[u8])]) -> Result<usize, String> {
        let mut out = MAGIC.to_vec();
        put_uv(&mut out, entries.len() as u64);
        for (k, b) in entries {
            out.extend_from_slice(&(*k as u64).to_le_bytes());
            put_bytes(&mut out, b);
        }
        let tmp = format!("{}.tmp", path.display());
        std::fs::write(&tmp, &out).map_err(|e| format!("{tmp}: {e}"))?;
        let a = format!("{tmp}\0");
        let b = format!("{}\0", path.display());
        // SAFETY: どちらも NUL で終わる文字列
        let r = unsafe { c_rename(a.as_bytes().as_ptr(), b.as_bytes().as_ptr()) };
        if r != 0 {
            return Err(format!("{}: rename failed", path.display()));
        }
        Ok(out.len())
    }
}

/// 関数への参照: 普通の関数は名前 (タグ 0)、インスタンスは (元の関数名, 型引数) (タグ 2)、vtable の空欄 (タグ 6)
fn put_func(out: &mut Vec<u8>, id: u32, prog: &Program, insts: &Insts) {
    if id == u32::MAX {
        out.push(6);
        return;
    }
    match insts.get(id) {
        None => {
            out.push(0);
            put_bytes(out, prog.names[id as usize].as_bytes());
        }
        Some(inst) => {
            out.push(2);
            put_bytes(out, prog.names[inst.base as usize].as_bytes());
            out.push(inst.args.len() as u8);
            for &t in &inst.args {
                crate::types_enc::put_ty(out, &prog.ty_syn(t));
            }
        }
    }
}

fn get_name<'a>(r: &mut Rd<'a>) -> Option<&'a str> {
    let n = r.uv()? as usize;
    let s = r.d.get(r.pos..r.pos + n)?;
    r.pos += n;
    std::str::from_utf8(s).ok()
}

/// put_func の逆 (タグを読んだ後)。インスタンスは insts に要求し直す (pending に積まれる)
fn get_func(r: &mut Rd, tag: u8, prog: &Program, insts: &Mutex<Insts>) -> Option<u32> {
    if tag == 6 {
        return Some(u32::MAX);
    }
    let name = get_name(r)?;
    let id = *prog.index.get(name)?;
    if tag == 0 {
        return Some(id);
    }
    let na = r.u8()? as usize;
    let mut args = Vec::with_capacity(na);
    for _ in 0..na {
        let mut pos = r.pos;
        let ts = crate::types_enc::read_ty(r.d, &mut pos)?;
        r.pos = pos;
        args.push(prog.resolve(&ts).ok()?);
    }
    insts.lock().unwrap().request(prog, id, args).ok()
}

/// 機械語 + 再配置 + 無名データ + vtable + panic の場所を書く (整数は可変長、オフセットと行は前からの差)
pub fn encode_blob(b: &Blob, prog: &Program, insts: &Insts, out: &mut Vec<u8>) {
    put_uv(out, b.align as u64);
    put_bytes(out, &b.code);
    put_uv(out, b.relocs.len() as u64);
    let mut prev = 0u32;
    for r in &b.relocs {
        put_sv(out, r.offset as i64 - prev as i64);
        prev = r.offset;
        out.push(r.kind);
        put_sv(out, r.addend);
        match &r.target {
            Target::Func(id) => put_func(out, *id, prog, insts),
            Target::SelfOffset(o) => {
                out.push(1);
                put_uv(out, *o as u64);
            }
            Target::Data(k) => {
                out.push(3);
                put_uv(out, *k as u64);
            }
            // static は名前で (番号はアイテムの並びで変わる)
            Target::Static(ci) => {
                out.push(4);
                put_bytes(out, prog.consts[*ci as usize].name.as_bytes());
            }
            Target::Vtable(k) => {
                out.push(5);
                put_uv(out, *k as u64);
            }
            Target::Sym(n) => {
                out.push(7);
                put_bytes(out, n.as_bytes());
            }
        }
    }
    put_uv(out, b.datas.len() as u64);
    for d in &b.datas {
        put_bytes(out, d);
    }
    put_uv(out, b.vtables.len() as u64);
    for vt in &b.vtables {
        put_uv(out, vt.len() as u64);
        for &f in vt {
            put_func(out, f, prog, insts);
        }
    }
    put_uv(out, b.traps.len() as u64);
    let (mut po, mut pl) = (0i64, 0i64);
    for &(o, info, m) in &b.traps {
        let (kind, line, col) = (info >> 27, (info >> 11) & 0xFFFF, info & 0x7FF);
        put_sv(out, o as i64 - po);
        po = o as i64;
        let m1 = if m == u32::MAX { 0 } else { m as u64 + 1 };
        put_uv(out, kind as u64 | (m1 << 4));
        put_sv(out, line as i64 - pl);
        pl = line as i64;
        put_uv(out, col as u64);
    }
    put_uv(out, b.line_base.map_or(0, |l| l as u64 + 1));
}

/// encode_blob の逆。呼び先のインスタンスは insts に要求し直す
pub fn decode_blob(d: &[u8], prog: &Program, insts: &Mutex<Insts>) -> Option<Blob> {
    let mut r = Rd { d, pos: 0 };
    let align = r.uv()? as u32;
    let n = r.uv()? as usize;
    let code = r.take(n)?.to_vec();
    let nr = r.uv()? as usize;
    let mut relocs = Vec::with_capacity(nr);
    let mut prev = 0i64;
    for _ in 0..nr {
        prev += r.sv()?;
        let offset = prev as u32;
        let kind = r.u8()?;
        let addend = r.sv()?;
        let tag = r.u8()?;
        let target = match tag {
            0 | 2 | 6 => Target::Func(get_func(&mut r, tag, prog, insts)?),
            1 => Target::SelfOffset(r.uv()? as u32),
            3 => Target::Data(r.uv()? as u32),
            4 => Target::Static(*prog.const_index.get(get_name(&mut r)?)?),
            5 => Target::Vtable(r.uv()? as u32),
            7 => Target::Sym(get_name(&mut r)?.to_string()),
            _ => return None,
        };
        relocs.push(BlobReloc { offset, kind, addend, target });
    }
    let nd = r.uv()? as usize;
    let mut datas = Vec::with_capacity(nd);
    for _ in 0..nd {
        let n = r.uv()? as usize;
        datas.push(r.take(n)?.to_vec());
    }
    let nv = r.uv()? as usize;
    let mut vtables = Vec::with_capacity(nv);
    for _ in 0..nv {
        let n = r.uv()? as usize;
        let mut vt = Vec::with_capacity(n);
        for _ in 0..n {
            let tag = r.u8()?;
            vt.push(get_func(&mut r, tag, prog, insts)?);
        }
        vtables.push(vt);
    }
    let nt = r.uv()? as usize;
    let mut traps = Vec::with_capacity(nt);
    let (mut po, mut pl) = (0i64, 0i64);
    for _ in 0..nt {
        po += r.sv()?;
        let km = r.uv()?;
        pl += r.sv()?;
        let col = r.uv()? as u32;
        let info = ((km & 0xF) as u32) << 27 | ((pl as u32) & 0xFFFF) << 11 | (col & 0x7FF);
        let m = (km >> 4) as u32;
        traps.push((po as u32, info, m.wrapping_sub(1)));
    }
    let line_base = match r.uv()? {
        0 => None,
        l => Some(l as u32 - 1),
    };
    Some(Blob { align, code, relocs, datas, vtables, traps, line_base, code_ref: None })
}
