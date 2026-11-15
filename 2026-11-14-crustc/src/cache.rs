//! 関数単位の機械語キャッシュ (1 ファイル)。
//!
//! - キー: 128bit = H(コンパイラ ID, 設定, 関数のトークン列, 参照先のシグネチャ)
//!   参照先の「本体」は入れない。呼び出し先を編集しても呼び出し元はヒットする。
//! - 値: 機械語 + シンボリックなリロケーション (参照先は名前で保存)、
//!   またはアイテムの要約 (シグネチャ)。種類はキーの導出時に区別する。
//! - 保存時は「今回のビルドで使ったエントリ」だけを書き戻す。
//!   古いエントリは自動的に消えるので、ストレージ消費は常に現在のコード量程度に収まる。
//! - 読み込み時はファイル全体を 1 回 read し、index だけ作る。値のデコードはヒット時のみ。

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;

use std::sync::Mutex;

use crate::codegen::{Blob, BlobReloc, Target};
use crate::front::{Insts, Program};

const MAGIC: &[u8; 8] = b"CRCC0004";

pub type Key = u128;

pub struct KeyHasher(std::hash::DefaultHasher, std::hash::DefaultHasher);

impl KeyHasher {
    pub fn new() -> Self {
        // DefaultHasher::new() は固定鍵の SipHash で、同じ std なら決定的。
        // std のバージョン差はコンパイラ ID (実行ファイル自体) をキーに含めることで吸収する。
        let a = std::hash::DefaultHasher::new();
        let mut b = std::hash::DefaultHasher::new();
        0xa5a5_u16.hash(&mut b);
        KeyHasher(a, b)
    }
    pub fn add<T: Hash + ?Sized>(&mut self, v: &T) {
        v.hash(&mut self.0);
        v.hash(&mut self.1);
    }
    pub fn finish(&self) -> Key {
        ((self.0.finish() as u128) << 64) | self.1.finish() as u128
    }
}

/// キャッシュファイル: MAGIC の後ろは zstd で圧縮した本体。本体は
/// [前回のオブジェクトのハッシュ (16 byte), エントリの数 (可変長), (キーの下位 64 bit, 長さ (可変長), 中身) *]。
/// (キーは 128 bit のハッシュだが、ファイルには下位 64 bit だけを書く: 数千エントリでの衝突は 2^-40 未満)
pub struct Cache {
    /// 解凍した本体
    data: Vec<u8>,
    index: HashMap<u64, (usize, usize)>,
    /// 前回の最終オブジェクトのハッシュ (リンク省略判定用)
    pub last_obj: Key,
}

struct Rd<'a>(&'a [u8], usize);
impl Rd<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.0.get(self.1..self.1 + n)?;
        self.1 += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn uv(&mut self) -> Option<u64> {
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
    fn sv(&mut self) -> Option<i64> {
        let u = self.uv()?;
        Some((u >> 1) as i64 ^ -((u & 1) as i64))
    }
    fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }
}

impl Cache {
    pub fn open(path: &Path) -> Cache {
        let file = std::fs::read(path).unwrap_or_default();
        let data = match file.get(..8) {
            Some(m) if m == MAGIC => {
                let n = zstd::bulk::Decompressor::upper_bound(&file[8..]).unwrap_or(64 << 20);
                zstd::bulk::decompress(&file[8..], n).unwrap_or_default()
            }
            _ => vec![],
        };
        let mut c = Cache {
            data: vec![],
            index: HashMap::new(),
            last_obj: 0,
        };
        if let Some((idx, last)) = Self::parse_index(&data) {
            c.index = idx;
            c.last_obj = last;
            c.data = data;
        }
        c
    }

    pub fn empty() -> Cache {
        Cache {
            data: vec![],
            index: HashMap::new(),
            last_obj: 0,
        }
    }

    fn parse_index(d: &[u8]) -> Option<(HashMap<u64, (usize, usize)>, Key)> {
        let mut r = Rd(d, 0);
        let last = r.u128()?;
        let n = r.uv()? as usize;
        let mut idx = HashMap::with_capacity(n);
        for _ in 0..n {
            let k = r.u64()?;
            let len = r.uv()? as usize;
            let start = r.1;
            r.take(len)?;
            idx.insert(k, (start, start + len));
        }
        Some((idx, last))
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn file_size(&self) -> usize {
        self.data.len()
    }

    pub fn get(&self, key: Key) -> Option<&[u8]> {
        let &(s, e) = self.index.get(&(key as u64))?;
        Some(&self.data[s..e])
    }

    /// 使ったエントリだけで新しいキャッシュファイルを書く (tmp + rename)。
    pub fn save(
        &self,
        path: &Path,
        entries: &[(Key, &[u8])],
        last_obj: Key,
    ) -> std::io::Result<usize> {
        let mut out = Vec::with_capacity(self.data.len());
        out.extend_from_slice(&last_obj.to_le_bytes());
        put_uv(&mut out, entries.len() as u64);
        for (k, b) in entries {
            out.extend_from_slice(&(*k as u64).to_le_bytes());
            put_uv(&mut out, b.len() as u64);
            out.extend_from_slice(b);
        }
        if out == self.data {
            // 変化なしなら書かない
            return Ok(std::fs::metadata(path).map(|m| m.len() as usize).unwrap_or(0));
        }
        let mut file = MAGIC.to_vec();
        file.extend_from_slice(&zstd::bulk::compress(&out, 1)?);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &file)?;
        std::fs::rename(&tmp, path)?;
        Ok(file.len())
    }
}

/// 機械語 + リロケーションを書く。呼び先がインスタンスなら (元の関数名, 型引数) で書くので、
/// 次のビルドでインスタンス番号が変わっても読み戻せる
/// 機械語のハッシュ (キャッシュは機械語の代わりにこれを持ち、中身は `.o` から引く)
pub fn code_hash(code: &[u8]) -> u64 {
    let mut h = KeyHasher::new();
    h.add(code);
    h.finish() as u64
}

/// 符号なしの可変長整数 (7 bit ずつ、上位ビットが続きの印)
fn put_uv(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// 符号つき (zigzag) の可変長整数
fn put_sv(out: &mut Vec<u8>, v: i64) {
    put_uv(out, ((v << 1) ^ (v >> 63)) as u64);
}

/// 機械語 + リロケーション + 無名データ + vtable + panic の場所を書く。整数はすべて可変長、
/// オフセットと行は前の値からの差で書く (1 関数あたり十数 byte + 1 か所あたり 4〜5 byte)。
/// code_ref: 機械語は書かずに (長さ, ハッシュ) だけ書く (同じ機械語が出力の `.o` にあるとき)
pub fn encode_blob(b: &Blob, prog: &Program, insts: &Insts, out: &mut Vec<u8>, code_ref: bool) {
    put_uv(out, b.align as u64);
    if code_ref {
        put_uv(out, 1);
        put_uv(out, b.code.len() as u64);
        out.extend_from_slice(&code_hash(&b.code).to_le_bytes());
    } else {
        put_uv(out, 0);
        put_uv(out, b.code.len() as u64);
        out.extend_from_slice(&b.code);
    }
    put_uv(out, b.relocs.len() as u64);
    let mut prev = 0u32;
    for r in &b.relocs {
        put_sv(out, r.offset as i64 - prev as i64);
        prev = r.offset;
        out.push(r.kind);
        put_sv(out, r.addend);
        match r.target {
            Target::Func(id) => put_func(out, id, prog, insts),
            Target::Vtable(k) => {
                out.push(5);
                put_uv(out, k as u64);
            }
            Target::SelfOffset(o) => {
                out.push(1);
                put_uv(out, o as u64);
            }
            Target::Data(k) => {
                out.push(3);
                put_uv(out, k as u64);
            }
            // static は名前で (番号はアイテムの並びで変わる)
            Target::Static(ci) => {
                out.push(4);
                let n = prog.consts[ci as usize].name.as_bytes();
                out.extend_from_slice(&(n.len() as u16).to_le_bytes());
                out.extend_from_slice(n);
            }
        }
    }
    put_uv(out, b.datas.len() as u64);
    for d in &b.datas {
        put_uv(out, d.len() as u64);
        out.extend_from_slice(d);
    }
    put_uv(out, b.vtables.len() as u64);
    for vt in &b.vtables {
        put_uv(out, vt.len() as u64);
        for &f in vt {
            put_func(out, f, prog, insts);
        }
    }
    // panic の場所: (オフセットの差, 種類とメッセージ番号, 行の差, 列)
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

/// 関数への参照: 普通の関数は名前 (タグ 0)、インスタンスは (元の関数名, 型引数) (タグ 2)
fn put_func(out: &mut Vec<u8>, id: u32, prog: &Program, insts: &Insts) {
    // vtable の空欄 (drop の要らない型の drop glue)
    if id == u32::MAX {
        out.push(6);
        return;
    }
    let put_name = |out: &mut Vec<u8>, id: u32| {
        let n = prog.names[id as usize].as_bytes();
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(n);
    };
    match insts.get(id) {
        None => {
            out.push(0);
            put_name(out, id);
        }
        Some(inst) => {
            out.push(2);
            put_name(out, inst.base);
            out.push(inst.args.len() as u8);
            for &t in &inst.args {
                crate::front::put_ty(out, &prog.ty_syn(t));
            }
        }
    }
}

/// put_func の逆 (タグを読んだ後)。インスタンスは insts に要求し直す
fn get_func(r: &mut Rd, tag: u8, prog: &Program, insts: &Mutex<Insts>) -> Option<u32> {
    if tag == 6 {
        return Some(u32::MAX);
    }
    let len = r.u16()? as usize;
    let name = std::str::from_utf8(r.take(len)?).ok()?;
    let id = *prog.index.get(name)?;
    if tag == 0 {
        return Some(id);
    }
    let na = r.u8()? as usize;
    let mut args = Vec::with_capacity(na);
    for _ in 0..na {
        let ts = crate::front::read_ty(r.0, &mut r.1)?;
        args.push(prog.resolve(&ts).ok()?);
    }
    insts.lock().unwrap().request(prog, id, args).ok()
}

/// インスタンスの列を (元の関数名, 型引数) で書く (check の「検査済み」の印に付ける)
/// 空なら何も書かない (大半の関数はインスタンスを要求しないので、印は 0 byte のまま)
pub fn encode_insts(ids: &[u32], prog: &Program, insts: &Insts, out: &mut Vec<u8>) {
    if ids.is_empty() {
        return;
    }
    out.extend_from_slice(&(ids.len() as u32).to_le_bytes());
    for &id in ids {
        let inst = insts.get(id).unwrap();
        let n = prog.names[inst.base as usize].as_bytes();
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(n);
        out.push(inst.args.len() as u8);
        for &t in &inst.args {
            crate::front::put_ty(out, &prog.ty_syn(t));
        }
    }
}

/// encode_insts の逆。各インスタンスを insts に要求し直す。読めなければ false
pub fn request_insts(d: &[u8], prog: &Program, insts: &Mutex<Insts>) -> bool {
    let mut r = Rd(d, 0);
    let Some(n) = r.u32() else {
        return d.is_empty();
    };
    for _ in 0..n {
        let ok = (|| {
            let len = r.u16()? as usize;
            let name = std::str::from_utf8(r.take(len)?).ok()?;
            let id = *prog.index.get(name)?;
            let na = r.u8()? as usize;
            let mut args = Vec::with_capacity(na);
            for _ in 0..na {
                let ts = crate::front::read_ty(r.0, &mut r.1)?;
                args.push(prog.resolve(&ts).ok()?);
            }
            insts.lock().unwrap().request(prog, id, args).ok()
        })();
        if ok.is_none() {
            return false;
        }
    }
    true
}

/// encode_blob の逆。インスタンスへの参照は insts に要求し直す (pending に積まれる)
pub fn decode_blob(d: &[u8], prog: &Program, insts: &Mutex<Insts>) -> Option<Blob> {
    let mut r = Rd(d, 0);
    let align = r.uv()? as u32;
    let (code, code_ref) = match r.uv()? {
        0 => {
            let n = r.uv()? as usize;
            (r.take(n)?.to_vec(), None)
        }
        _ => {
            let len = r.uv()? as u32;
            (vec![], Some((len, r.u64()?)))
        }
    };
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
            0 | 2 => Target::Func(get_func(&mut r, tag, prog, insts)?),
            5 => Target::Vtable(r.uv()? as u32),
            1 => Target::SelfOffset(r.uv()? as u32),
            3 => Target::Data(r.uv()? as u32),
            4 => {
                let len = r.u16()? as usize;
                let name = std::str::from_utf8(r.take(len)?).ok()?;
                Target::Static(*prog.const_index.get(name)?)
            }
            _ => return None,
        };
        relocs.push(BlobReloc {
            offset,
            kind,
            addend,
            target,
        });
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
    Some(Blob {
        align,
        code,
        relocs,
        datas,
        vtables,
        traps,
        line_base,
        code_ref,
    })
}
