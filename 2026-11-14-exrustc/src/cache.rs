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

const MAGIC: &[u8; 8] = b"EXRC0001";

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

pub struct Cache {
    data: Vec<u8>,
    index: HashMap<Key, (usize, usize)>,
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
    fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }
}

impl Cache {
    pub fn open(path: &Path) -> Cache {
        let data = std::fs::read(path).unwrap_or_default();
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

    fn parse_index(d: &[u8]) -> Option<(HashMap<Key, (usize, usize)>, Key)> {
        let mut r = Rd(d, 0);
        if r.take(8)? != MAGIC {
            return None;
        }
        let last = r.u128()?;
        let n = r.u32()? as usize;
        let mut idx = HashMap::with_capacity(n);
        for _ in 0..n {
            let k = r.u128()?;
            let len = r.u32()? as usize;
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
        let &(s, e) = self.index.get(&key)?;
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
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&last_obj.to_le_bytes());
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (k, b) in entries {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        if out == self.data {
            return Ok(out.len()); // 変化なしなら書かない
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &out)?;
        std::fs::rename(&tmp, path)?;
        Ok(out.len())
    }
}

/// 機械語 + リロケーションを書く。呼び先がインスタンスなら (元の関数名, 型引数) で書くので、
/// 次のビルドでインスタンス番号が変わっても読み戻せる
pub fn encode_blob(b: &Blob, prog: &Program, insts: &Insts, out: &mut Vec<u8>) {
    out.extend_from_slice(&b.align.to_le_bytes());
    out.extend_from_slice(&(b.code.len() as u32).to_le_bytes());
    out.extend_from_slice(&b.code);
    out.extend_from_slice(&(b.relocs.len() as u32).to_le_bytes());
    for r in &b.relocs {
        out.extend_from_slice(&r.offset.to_le_bytes());
        out.push(r.kind);
        out.extend_from_slice(&r.addend.to_le_bytes());
        match r.target {
            Target::Func(id) => put_func(out, id, prog, insts),
            Target::Vtable(k) => {
                out.push(5);
                out.extend_from_slice(&k.to_le_bytes());
            }
            Target::SelfOffset(o) => {
                out.push(1);
                out.extend_from_slice(&o.to_le_bytes());
            }
            Target::Data(k) => {
                out.push(3);
                out.extend_from_slice(&k.to_le_bytes());
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
    out.extend_from_slice(&(b.datas.len() as u32).to_le_bytes());
    for d in &b.datas {
        out.extend_from_slice(&(d.len() as u32).to_le_bytes());
        out.extend_from_slice(d);
    }
    // vtable と panic の trap の表は (ほとんどの関数にはないので) あるときだけ書く
    let tail = !b.vtables.is_empty() || !b.traps.is_empty() || b.line_base.is_some();
    if tail {
        out.extend_from_slice(&(b.vtables.len() as u32).to_le_bytes());
        for vt in &b.vtables {
            out.extend_from_slice(&(vt.len() as u32).to_le_bytes());
            for &f in vt {
                put_func(out, f, prog, insts);
            }
        }
    }
    if !b.traps.is_empty() || b.line_base.is_some() {
        out.extend_from_slice(&(b.traps.len() as u32).to_le_bytes());
        // (1 か所 8 byte: オフセット 20 bit | メッセージ番号 + 1 13 bit | 種類・行・列 31 bit。表と同じ形)
        for &(o, i, m) in &b.traps {
            let m1 = if m >= (1 << 13) - 1 { 0 } else { m as u64 + 1 };
            let w = ((o as u64) << 44) | (m1 << 31) | (i as u64 & 0x7FFF_FFFF);
            out.extend_from_slice(&w.to_le_bytes());
        }
    }
    if let Some(lb) = b.line_base {
        out.extend_from_slice(&lb.to_le_bytes());
    }
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
    let align = r.u32()?;
    let n = r.u32()? as usize;
    let code = r.take(n)?.to_vec();
    let nr = r.u32()? as usize;
    let mut relocs = Vec::with_capacity(nr);
    for _ in 0..nr {
        let offset = r.u32()?;
        let kind = r.u8()?;
        let addend = r.u64()? as i64;
        let tag = r.u8()?;
        let target = match tag {
            0 | 2 => Target::Func(get_func(&mut r, tag, prog, insts)?),
            5 => Target::Vtable(r.u32()?),
            1 => Target::SelfOffset(r.u32()?),
            3 => Target::Data(r.u32()?),
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
    let nd = r.u32()? as usize;
    let mut datas = Vec::with_capacity(nd);
    for _ in 0..nd {
        let n = r.u32()? as usize;
        datas.push(r.take(n)?.to_vec());
    }
    let mut vtables = vec![];
    if r.1 < r.0.len() {
        let nv = r.u32()? as usize;
        for _ in 0..nv {
            let n = r.u32()? as usize;
            let mut vt = Vec::with_capacity(n);
            for _ in 0..n {
                let tag = r.u8()?;
                vt.push(get_func(&mut r, tag, prog, insts)?);
            }
            vtables.push(vt);
        }
    }
    let mut traps = vec![];
    if r.1 < r.0.len() {
        let n = r.u32()? as usize;
        for _ in 0..n {
            let w = r.u64()?;
            let m1 = ((w >> 31) & 0x1FFF) as u32;
            traps.push(((w >> 44) as u32, (w & 0x7FFF_FFFF) as u32, m1.wrapping_sub(1)));
        }
    }
    let line_base = if r.1 < r.0.len() { Some(r.u32()?) } else { None };
    Some(Blob {
        align,
        code,
        relocs,
        datas,
        vtables,
        traps,
        line_base,
    })
}
