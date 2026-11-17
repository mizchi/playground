//! syn と Cranelift を使わないドライバ: ソース → (lexer / macros / prelude) → ast_summary → Program →
//! acg (自前の構文木のコード生成) → minclif の ELF の .o → リンク。
//! driver.rs と同じ手順を、外部クレートに依存しないモジュールだけで行う (crustc 自身でもコンパイルできる)。
//! キャッシュと並列化はまだない。

use std::sync::Mutex;

use crate::acg::{self, Blob, ItemFn};
use crate::key::{Key, KeyHasher};
use crate::fxhash::{FxMap, FxSet};
use crate::lexer::{self, ItemKind, SrcItem};
use crate::parse::{Block, Expr, Pat, Stmt};
use crate::front::{FrontMemo, with_prelude_in};
use crate::prof::{Timer, now_ns};
use crate::program::{Insts, Program, TyEnv};
use crate::types::{Summary, Ty};

pub type R<T> = Result<T, String>;

/// ビルド (機械語・オブジェクト・リンク・ワークスペース)。feature `build` (既定) のときだけ
#[cfg(feature = "build")]
mod build;
#[cfg(feature = "build")]
pub use build::*;

/// check の記録 (LSP の編集をまたいで): 宣言の指紋と、検査に通った関数・インスタンス (名前 → 記録)
pub struct ChkMemo {
    fp: u64,
    ents: FxMap<String, ChkEnt>,
}

#[derive(Clone)]
struct ChkEnt {
    /// 本体の関数の名前と、そのアイテムのハッシュ
    body: String,
    body_hash: Key,
    /// 要求したインスタンスの名前
    children: Vec<String>,
    /// インスタンスなら、自分を要求し直すための Blob (自分への再配置だけ)
    me: Vec<u8>,
    /// 検査に通ったか (通らなかったものは毎回検査し直す)
    ok: bool,
}


/// 解析の結果
pub struct Analysis {
    /// 宣言の指紋: fn アイテムは要約 (シグネチャ・クロージャ)、ほかのアイテムはテキストのハッシュを並べたもの。
    /// fn の本体だけを変えた編集では変わらない (check の記憶 ChkMemo を使ってよいかの印)
    pub iface_fp: u64,
    pub src: String,
    pub items: Vec<SrcItem>,
    pub prog: Program,
    pub refs: Vec<Vec<u32>>,
    /// 関数ごとの、名前で現れる struct と const / static (キャッシュのキー用)
    pub srefs: Vec<Vec<u32>>,
    pub crefs: Vec<Vec<u32>>,
    /// 要約を作り直した (キャッシュになかった) アイテムの数
    pub n_parsed: usize,
    /// アイテムごとの、依存するクレートのものか (CrateCtx)
    pub ext_items: Vec<bool>,
    /// `-O` で展開してよい (prelude の小さい) 関数の本体
    pub inl: acg::Inline,
    /// 機械語にするバックエンド (None なら minclif)
    pub cl: Option<Backend>,
    /// prelude の先頭の行 (結合したソースの行)
    pub prelude_line0: usize,
    /// 読んだ関数の本体 (関数 id → 構文木)
    pub asts: Mutex<FxMap<u32, ItemFn>>,
    /// 囲む関数 (アイテム番号) ごとのクロージャの式 (クロージャの本体を引くたびに囲む関数を読み直さない)
    pub encl: Mutex<FxMap<u32, Vec<Expr>>>,
    /// 関数ごとのシグネチャのハッシュ (0 はまだ: キーを作るたびに参照先のシグネチャをたどり直さない)
    pub sig_keys: Mutex<Vec<Key>>,
    /// 依存するクレートのテキストの範囲と、そこから見えるクレート (CrateCtx::dep_ranges)
    pub dep_ranges: Vec<(u32, u32, Vec<usize>, u32)>,
}

/// クレートの印 (panic の場所の行の上位ビット。ライブラリの panic の表の 8 byte 目): 1..=4000
pub fn crate_tag(name: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    1 + h % 4000
}

/// prelude の中の panic の場所の行の基準 (結合したソースの行によらない: prelude の関数の機械語をクレートの間で使い回す)。
/// 場所の表では、この行から後ろは prelude のファイル
pub const PRELUDE_LINE: u32 = 49152;

/// 検査だけの印のキャッシュのキー (ビルドの Blob のキーと混ざらないように)
const CHECK_SALT: Key = 0x5eed_c4ec_0000_0000_0000_0000_0000_0001;

impl Analysis {
    /// アイテム ii が依存するクレートのものなら、その dep_ranges の番号
    /// シグネチャのハッシュ (覚えておく)
    pub fn sig_key(&self, id: u32) -> Key {
        let mut ks = self.sig_keys.lock().unwrap();
        if ks.is_empty() {
            ks.resize(self.prog.names.len(), 0);
        }
        let k = ks[id as usize];
        if k != 0 {
            return k;
        }
        let mut h = KeyHasher::new();
        self.prog.hash_sig(id, &mut h);
        let k = h.finish() | 1;
        ks[id as usize] = k;
        k
    }

    pub fn crate_of_item(&self, ii: usize) -> Option<usize> {
        if ii >= self.prog.prelude_from as usize || !self.ext_items.get(ii).copied().unwrap_or(false) {
            return None;
        }
        let l = self.items[ii].line as u32;
        self.dep_ranges.iter().position(|r| r.0 <= l && l < r.1)
    }

    /// 閉じた関数 (インスタンス): 本体が prelude か依存するクレートにあり、型引数にこのクレートの型を含まないもの。
    /// 本体が呼びうるのは、本体のクレートと型引数の型のクレートから見えるクレート (と prelude) のものだけなので、
    /// キャッシュのキーはそれらだけで決まる (使う側によらない)。返り値は見えるクレート (dep_ranges の番号ごと)
    pub fn closed_univ(&self, insts: &Insts, id: u32) -> Option<Vec<bool>> {
        let body = insts.body_of(id);
        if self.prog.closures.contains_key(&body) {
            return None;
        }
        let ii = self.prog.item_of[body as usize]? as usize;
        let mut u = vec![false; self.dep_ranges.len()];
        if ii < self.prog.prelude_from as usize {
            let r = self.crate_of_item(ii)?;
            for &x in &self.dep_ranges[r].2 {
                u[x] = true;
            }
        }
        if let Some(inst) = insts.get(id) {
            let mut stack: Vec<Ty> = inst.args.clone();
            while let Some(t) = stack.pop() {
                match t {
                    Ty::Ref(_, i) | Ty::Ptr(_, i) | Ty::Array(i, _) | Ty::Slice(i) | Ty::Box(i) => stack.push(*i),
                    Ty::Struct(sid) => {
                        let b = self.prog.base_of(sid);
                        if let Some(&sii) = self.prog.struct_item.get(b as usize) {
                            let sii = sii as usize;
                            if sii < self.prog.prelude_from as usize {
                                let c = self.crate_of_item(sii)?;
                                for &x in &self.dep_ranges[c].2 {
                                    u[x] = true;
                                }
                            }
                        }
                        stack.extend(self.prog.structs[sid as usize].args.iter().copied());
                    }
                    Ty::Fn(f) => {
                        stack.extend(f.params.iter().copied());
                        stack.push(f.ret);
                    }
                    Ty::Dyn(_) | Ty::Assoc(_) | Ty::Param(_) => return None,
                    _ => {}
                }
            }
        }
        Some(u)
    }

    /// 関数 (インスタンス) id の panic の場所の行の基準。閉じた依存するクレートの関数は、そのクレートのテキストの先頭からの
    /// 行 (使う側の結合したソースの中の位置によらない: .o を書くときに位置を足す)
    pub fn line_base_for(&self, insts: &Insts, id: u32) -> Option<u32> {
        let body = insts.body_of(id);
        let ii = self.prog.item_of[body as usize]? as usize;
        if let Some(r) = self.crate_of_item(ii) {
            if self.closed_univ(insts, id).is_some() {
                return Some((self.dep_ranges[r].3 << 20) | (self.items[ii].line as u32 - self.dep_ranges[r].0));
            }
        }
        Some(self.line_base(ii))
    }

    /// .o に書くときに、関数 id の panic の場所の行に足す数 (line_base_for で依存するクレートの中の行にしたもの)
    pub fn line_shift(&self, insts: &Insts, id: u32) -> u32 {
        let body = insts.body_of(id);
        let Some(ii) = self.prog.item_of.get(body as usize).copied().flatten() else { return 0 };
        match self.crate_of_item(ii as usize) {
            Some(r) if self.closed_univ(insts, id).is_some() => self.dep_ranges[r].0 - 1,
            _ => 0,
        }
    }

    /// アイテム ii の panic の場所の行の基準 (アイテムの先頭の行 - 1)
    pub fn line_base(&self, ii: usize) -> u32 {
        let it = &self.items[ii];
        let user_lines = self.prelude_line0 as u32;
        if ii >= self.prog.prelude_from as usize && self.prelude_line0 > 0 && user_lines < PRELUDE_LINE && it.line >= self.prelude_line0 {
            PRELUDE_LINE + (it.line - self.prelude_line0).saturating_sub(1) as u32
        } else {
            it.line as u32 - 1
        }
    }
}

impl Analysis {
    /// 依存するクレートの .o にある関数 (ここではコンパイルしない)
    pub fn is_ext(&self, id: u32) -> bool {
        concrete(&self.prog, id as usize) && self.prog.item_of[id as usize].is_some_and(|ii| self.ext_items[ii as usize])
    }
    /// 依存するクレートの static
    pub fn is_ext_static(&self, ci: u32) -> bool {
        self.ext_items.get(self.prog.consts[ci as usize].item as usize).copied().unwrap_or(false)
    }
}

pub fn analyze(src0: &str, opt: bool) -> R<Analysis> {
    Session2::new(Options2 { opt, no_cache: true, jobs: 1, ..Default::default() }).analyze(src0)
}

/// 機械語にするバックエンド (Cranelift: crustc-clif の compile)。feature `build` なしでは使わない
#[cfg(feature = "build")]
pub type Backend = crate::lower::ClBackend;
#[cfg(not(feature = "build"))]
pub type Backend = fn();

/// ドライバの設定
#[derive(Clone, Debug)]
pub struct Options2 {
    pub opt: bool,
    /// 関数のコンパイルに使うプロセスの数 (fork する)
    pub jobs: usize,
    pub linker: Option<String>,
    pub no_cache: bool,
    /// 段階ごとの時間を stderr に出す
    pub time: bool,
    /// minclif の代わりのバックエンド (Cranelift: crate::clif_backend::compile)
    pub backend: Option<Backend>,
    /// `#[cfg(feature = "..")]` で真になる feature (1 つのクレートのビルド。ワークスペースはクレートごとに CrateSpec が持つ)
    pub features: Vec<String>,
}

impl Default for Options2 {
    fn default() -> Self {
        Options2 { opt: false, jobs: cpu_count(), linker: None, no_cache: false, time: false, backend: None, features: vec![] }
    }
}

impl Options2 {
    /// キャッシュのキーに入れるバックエンドの印 (Cranelift なら 1)
    pub fn backend_id(&self) -> u8 {
        self.backend.is_some() as u8
    }

    /// cfg の述語の設定 (features は与えたもの)
    pub fn cfg_with(&self, features: &[String]) -> crate::cfg::CfgSet {
        crate::cfg::CfgSet { features: features.to_vec(), debug_assertions: !self.opt }
    }
}

/// 複数のクレートのうちの 1 つをコンパイルするときの情報。ソースは「このクレートのコード (1..=own_lines 行)」の後ろに
/// 依存するクレートを `pub mod name { .. }` として付けたもの。依存するクレートの (総称でない) 関数と static は
/// そのクレートの .o にあるので、ここではコンパイルせずシンボルで参照する
#[derive(Clone, Debug, Default)]
pub struct CrateCtx {
    /// ライブラリをコンパイルするときはその名前 (総称でない関数と static を外に見せる)
    pub lib: Option<String>,
    /// このクレートのコードの行数 (0 なら全部)
    pub own_lines: u32,
    /// 結合したソースの行 → ファイル
    pub map: crate::srcmap::SrcMap,
    /// 依存するクレート (推移的に全部。実行ファイルの panic の表がそれらの表を指す)
    pub deps: Vec<String>,
    /// このクレートの feature (ワークスペース。None なら Options2 の features)
    pub features: Option<Vec<String>>,
    /// 依存するクレートのテキストの範囲 (結合したソースの行 [開始, 終わり)) と、そこから見えるクレート (この表の番号。自分も)
    pub dep_ranges: Vec<(u32, u32, Vec<usize>, u32)>,
}

/// ビルドの結果
#[derive(Clone, Debug, Default)]
pub struct Report2 {
    pub items_parsed: usize,
    pub fns_compiled: usize,
    pub fns_hit: usize,
    pub object_bytes: usize,
    pub linked: bool,
}

// (read / write は std と同じ名前の libc の関数。ポインタを *mut u8 で受ける: crustc でもコンパイルできるように)
#[allow(suspicious_runtime_symbol_definitions)]
unsafe extern "C" {
    #[link_name = "sysconf"]
    fn c_sysconf(name: i32) -> i64;
    #[link_name = "stat"]
    fn c_stat(path: *const u8, buf: *mut u8) -> i32;
    #[link_name = "fork"]
    fn c_fork() -> i32;
    #[link_name = "pipe"]
    fn c_pipe(fds: *mut i32) -> i32;
    #[link_name = "read"]
    fn c_read(fd: i32, buf: *mut u8, n: usize) -> isize;
    #[link_name = "write"]
    fn c_write(fd: i32, buf: *const u8, n: usize) -> isize;
    #[link_name = "close"]
    fn c_close(fd: i32) -> i32;
    #[link_name = "waitpid"]
    fn c_waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    #[cfg(feature = "build")]
    #[link_name = "mkdir"]
    fn c_mkdir(path: *const u8, mode: u32) -> i32;
    #[link_name = "_exit"]
    fn c_exit(code: i32) -> !;
}

/// 使える CPU の数
pub fn cpu_count() -> usize {
    // SAFETY: _SC_NPROCESSORS_ONLN (84)
    let n = unsafe { c_sysconf(84) };
    if n < 1 { 1 } else { n as usize }
}

/// 単調時計 (ナノ秒)

/// コンパイラ自身の識別 (実行ファイルの大きさと更新時刻): 作り直したらキャッシュを使わない
fn compiler_id() -> Key {
    let mut buf = [0u8; 256];
    // SAFETY: "/proc/self/exe" は NUL で終わる。buf は struct stat (144 byte) より大きい
    let r = unsafe { c_stat(b"/proc/self/exe\0".as_ptr(), buf.as_mut_ptr()) };
    let mut h = KeyHasher::new();
    h.add("driver2-1");
    if r == 0 {
        // (x86-64: st_size は 48、st_mtim は 88 / 96)
        h.add(&buf[48..56]);
        h.add(&buf[88..104]);
    }
    h.finish()
}

/// 時間の計測 (-time)

/// キャッシュを持つドライバ (driver.rs の Session と同じ使い方)
pub struct Session2 {
    pub opts: Options2,
    cid: Key,
    disk: crate::cache2::Cache2,
    mem: FxMap<Key, Vec<u8>>,
    used: FxSet<Key>,
    /// put したキー (ワークスペースの子プロセスが、作ったエントリを親に返す)
    fresh: Vec<Key>,
    /// prelude の選択の、アイテムごとの記憶 (LSP の編集のたびに全アイテムを調べ直さない)
    sel: FrontMemo,
    /// fn アイテムのハッシュ → 要約の符号のハッシュ (Analysis::iface_fp を作るため)
    sum_fp: FxMap<Key, u64>,
    /// 前回の check の記録
    chk: Option<ChkMemo>,
    /// 複数のクレートのうちの 1 つをコンパイルするとき
    pub ctx: CrateCtx,
}

impl Session2 {
    pub fn new(opts: Options2) -> Session2 {
        Session2 { opts, cid: compiler_id(), disk: crate::cache2::Cache2::empty(), mem: FxMap::default(), used: FxSet::default(), fresh: vec![], ctx: CrateCtx::default(), sel: Default::default(), sum_fp: FxMap::default(), chk: None }
    }

    pub fn load_cache(&mut self, path: &std::path::Path) {
        if !self.opts.no_cache {
            self.disk = crate::cache2::Cache2::open(path);
        }
    }

    /// 今回使ったエントリだけを書く
    pub fn save_cache(&mut self, path: &std::path::Path) -> Result<usize, String> {
        // (今回使ったものはディスクの上の古い表にしかなくてもメモリに移す: 書いたファイルを開き直さないので)
        let used = self.used.clone();
        for &k in &used {
            if !self.mem.contains_key(&k) {
                if let Some(v) = self.disk.get(k) {
                    let v = v.to_vec();
                    self.mem.insert(k, v);
                }
            }
        }
        let n = self.save_cache_keep(path)?;
        self.mem.retain(|k, _| used.contains(k));
        self.disk = crate::cache2::Cache2::empty();
        Ok(n)
    }

    /// save_cache と同じだが、メモリの上のエントリは残す (ワークスペースで次のクレートが使う)
    fn save_cache_keep(&mut self, path: &std::path::Path) -> Result<usize, String> {
        if self.opts.no_cache {
            return Ok(0);
        }
        let mut keys: Vec<Key> = self.used.iter().copied().collect();
        keys.sort_unstable();
        let mut entries: Vec<(Key, Vec<u8>)> = vec![];
        for k in keys {
            let v = match self.mem.get(&k) {
                Some(v) => v.clone(),
                None => match self.disk.get(k) {
                    Some(v) => v.to_vec(),
                    None => continue,
                },
            };
            entries.push((k, v));
        }
        let refs: Vec<(Key, &[u8])> = entries.iter().map(|(k, v)| (*k, &v[..])).collect();
        let n = crate::cache2::Cache2::save(path, &refs)?;
        self.used.clear();
        Ok(n)
    }

    fn get(&mut self, k: Key) -> Option<Vec<u8>> {
        if self.opts.no_cache {
            return None;
        }
        self.used.insert(k);
        match self.mem.get(&k) {
            Some(v) => Some(v.clone()),
            None => self.disk.get(k).map(|v| v.to_vec()),
        }
    }

    fn put(&mut self, k: Key, v: Vec<u8>) {
        if self.opts.no_cache {
            return;
        }
        self.used.insert(k);
        self.mem.insert(k, v);
        self.fresh.push(k);
    }

    /// 要約はアイテムのハッシュで引く
    pub fn analyze(&mut self, src0: &str) -> R<Analysis> {
        // (ファイルから読んだもの (load_crate) は読むときに cfg を評価してある)
        let stripped;
        let src0 = if self.ctx.map.inputs.is_empty() {
            match crate::cfg::strip(src0, &self.opts.cfg_with(&self.opts.features)) {
                Some(t) => {
                    stripped = t;
                    stripped.as_str()
                }
                None => src0,
            }
        } else {
            src0
        };
        let mut crates = self.ctx.deps.clone();
        crates.extend(self.ctx.lib.iter().cloned());
        let mut fix = vec![];
        let cfg = self.opts.cfg_with(self.ctx.features.as_ref().unwrap_or(&self.opts.features));
        let (src, items, user, prelude_line0) = with_prelude_in(src0, &crates, &mut fix, Some(&cfg), &mut self.sel)?;
        self.ctx.map.colfix = fix;
        let mut tm = Timer { on: std::env::var_os("CRUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
        if let Some(p) = std::env::var_os("CRUSTC_DUMP_SRC") {
            let _ = std::fs::write(p, &src);
        }
        let cid = self.cid;
        let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
        let mut n_parsed = 0;
        for pass in 0..2 {
            // (キャッシュにないもの: (番号, キー))
            let mut miss: Vec<(usize, Key)> = vec![];
            for (i, it) in items.iter().enumerate() {
                let header = matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader);
                if header != (pass == 0) {
                    continue;
                }
                let mut h = KeyHasher::new();
                h.add(&cid);
                h.add(&0u8);
                h.add(&it.hash);
                let k = h.finish();
                if let Some(sm) = self.get(k).and_then(|d| Summary::decode(&d)) {
                    sums[i] = sm;
                    continue;
                }
                miss.push((i, k));
            }
            // (多ければ fork して分けて作る: 子 k は j % jobs == k のものを作り、直列化して返す)
            let jobs = self.opts.jobs.min(miss.len() / 32).max(1);
            let mut got: Vec<Option<Summary>> = vec![None; miss.len()];
            if jobs > 1 {
                let mut kids: Vec<(i32, i32)> = vec![];
                for kk in 0..jobs {
                    let mut fds = [0i32; 2];
                    // SAFETY: fds は 2 つの int
                    if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                        continue;
                    }
                    let pid = unsafe { c_fork() };
                    if pid == 0 {
                        unsafe { c_close(fds[0]) };
                        let mut buf = vec![];
                        let mut j = kk;
                        while j < miss.len() {
                            let it = &items[miss[j].0];
                            let ow = if pass == 1 { crate::ast_summary::owner_of(it, &sums) } else { None };
                            // (失敗したものは送らない: 親が作り直してエラーを出す)
                            if let Ok(sm) = crate::ast_summary::parse_and_summarize(&src, it, ow.as_ref()) {
                                let mut e = vec![];
                                sm.encode(&mut e);
                                crate::cache2::put_uv(&mut buf, j as u64);
                                crate::cache2::put_uv(&mut buf, e.len() as u64);
                                buf.extend_from_slice(&e);
                            }
                            j += jobs;
                        }
                        write_all(fds[1], &buf);
                        unsafe { c_close(fds[1]) };
                        unsafe { c_exit(0) };
                    }
                    unsafe { c_close(fds[1]) };
                    if pid < 0 {
                        unsafe { c_close(fds[0]) };
                        continue;
                    }
                    kids.push((pid, fds[0]));
                }
                for (pid, fd) in kids {
                    let d = read_all(fd);
                    unsafe { c_close(fd) };
                    let mut st = 0i32;
                    unsafe { c_waitpid(pid, &mut st, 0) };
                    let mut r = crate::cache2::Rd { d: &d, pos: 0 };
                    while r.pos < d.len() {
                        let (Some(j), Some(n)) = (r.uv(), r.uv()) else { break };
                        let Some(body) = r.take(n as usize) else { break };
                        if let Some(slot) = got.get_mut(j as usize) {
                            *slot = Summary::decode(body);
                        }
                    }
                }
            }
            for (j, &(i, k)) in miss.iter().enumerate() {
                let it = &items[i];
                let sm = match got[j].take() {
                    Some(sm) => sm,
                    None => {
                        let ow = if pass == 1 { crate::ast_summary::owner_of(it, &sums) } else { None };
                        crate::ast_summary::parse_and_summarize(&src, it, ow.as_ref()).map_err(|e| relocate(e, it))?
                    }
                };
                n_parsed += 1;
                let mut b = vec![];
                sm.encode(&mut b);
                sums[i] = sm;
                self.put(k, b);
            }
        }
        // 宣言の指紋 (要約の符号のハッシュはアイテムのハッシュで覚える)
        let iface_fp = {
            let mut h = KeyHasher::new();
            h.add(&self.ctx.dep_ranges.len());
            let mut new_fp: FxMap<Key, u64> = FxMap::default();
            for (i, it) in items.iter().enumerate() {
                let is_fn = !sums[i].fns.is_empty() && sums[i].structs.is_empty() && sums[i].enums.is_empty() && matches!(it.kind, ItemKind::Normal | ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
                if !is_fn || self.opts.opt {
                    h.add(&it.hash);
                    continue;
                }
                let x = match self.sum_fp.get(&it.hash) {
                    Some(&x) => x,
                    None => {
                        let mut e = vec![];
                        sums[i].encode(&mut e);
                        let mut hh = KeyHasher::new();
                        hh.add(&e[..]);
                        hh.finish() as u64
                    }
                };
                new_fp.insert(it.hash, x);
                h.add(&x);
            }
            self.sum_fp = new_fp;
            h.finish() as u64
        };
        tm.lap("summaries", {
            if tm.on {
                let ol = self.ctx.own_lines as usize;
                let own = items[..user].iter().filter(|it| ol == 0 || it.line <= ol).count();
                format!("{} items: own {own}, deps {}, prelude {}", items.len(), user - own, items.len() - user)
            } else {
                String::new()
            }
        });
        let mut prog = Program::build_with(&items, &sums, self.ctx.lib.is_none())?;
        tm.lap("program", String::new());
        prog.prelude_from = user as u32;
        prog.overflow_checks = !self.opts.opt;
        prog.opt = self.opts.opt;
        resolve_opaque(&mut prog, &src, &items)?;
        let ol = self.ctx.own_lines as usize;
        let ext_items: Vec<bool> = items.iter().enumerate().map(|(i, it)| ol > 0 && i < user && it.line > ol).collect();
        // (ライブラリ: 自分の総称でない関数は全部外に見せる (使う側のクレートが呼ぶ))
        if self.ctx.lib.is_some() {
            for id in 0..prog.names.len() {
                if concrete(&prog, id) && prog.item_of[id].is_some_and(|ii| (ii as usize) < user && !ext_items[ii as usize]) {
                    prog.sigs[id].export = true;
                }
            }
        }
        let mut refs = Vec::with_capacity(prog.item_of.len());
        let mut srefs = Vec::with_capacity(prog.item_of.len());
        let mut crefs = Vec::with_capacity(prog.item_of.len());
        let names = prog.ref_names();
        for ii in &prog.item_of {
            let (f, st, c) = ii.map(|ii| prog.refs_in(&src, &items[ii as usize], Some(&names))).unwrap_or_default();
            refs.push(f);
            srefs.push(st);
            crefs.push(c);
        }
        tm.lap("refs", String::new());
        let mut a = Analysis { iface_fp, src, items, prog, refs, srefs, crefs, n_parsed, ext_items, inl: acg::Inline::default(), cl: self.opts.backend, prelude_line0, dep_ranges: self.ctx.dep_ranges.clone(), asts: Mutex::new(FxMap::default()), encl: Mutex::new(FxMap::default()), sig_keys: Mutex::new(Vec::new()) };
        if self.opts.opt && std::env::var_os("CRUSTC_NO_INLINE").is_none() {
            a.inl = inline_bodies(&a);
        }
        Ok(a)
    }
}

/// `-> impl Iterator<..>` の戻り値を本体の型推論で決める (front.rs の resolve_opaque と同じ)
fn resolve_opaque(prog: &mut Program, src: &str, items: &[SrcItem]) -> R<()> {
    if prog.opaque.is_empty() {
        return Ok(());
    }
    let ids = prog.opaque.clone();
    let mut done = vec![false; ids.len()];
    for _ in 0..4 {
        let mut progress = false;
        for (k, &id) in ids.iter().enumerate() {
            if done[k] {
                continue;
            }
            let sig = prog.sigs[id as usize].clone();
            if sig.is_generic() {
                return Err(format!("`{}`: generic functions returning `impl Trait` are not supported", prog.names[id as usize]));
            }
            let Some(ii) = prog.item_of[id as usize] else { continue };
            let Some(f) = parse_fn(src, &items[ii as usize])? else { continue };
            let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty };
            let (_, r, _) = crate::ast_infer::infer_ctx(prog, f.sig.recv.is_some(), &f.sig.params, &f.block, &sig, &env, &[], None, Some((id, 0)));
            if let Some(t) = r.filter(|&t| t != Ty::Unit && !prog.has_param(t)) {
                prog.sigs[id as usize].ret = t;
                done[k] = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    if let Some(k) = done.iter().position(|d| !*d) {
        return Err(format!("cannot infer the `impl Trait` return type of `{}`", prog.names[ids[k] as usize]));
    }
    Ok(())
}

/// アイテムの中の `行:桁:` のエラーを、ファイルの中の行にする (driver.rs の relocate_err と同じ)。位置がなければアイテムの先頭の行
fn relocate(e: String, it: &SrcItem) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    format!("{}:1: {e}", it.line)
}

/// アイテムの関数を自前の構文木で読む (本体がなければ None)。引数のパターンは本体の先頭の let にする
pub fn parse_fn(src: &str, it: &SrcItem) -> R<Option<ItemFn>> {
    let text = &src[it.start..it.end];
    let f = crate::parse::parse_file(text).map_err(|e| relocate(e, it))?;
    let Some(item) = f.items.into_iter().next() else { return Ok(None) };
    let crate::parse::ItemKind::Fn(mut sig, Some(mut block)) = item.kind else { return Ok(None) };
    let mut lets = vec![];
    for (k, (p, _)) in sig.params.iter_mut().enumerate() {
        if matches!(p, Pat::Ident(..) | Pat::Wild) {
            continue;
        }
        let name = format!("__exr_arg{k}");
        let pat = std::mem::replace(p, Pat::Ident(false, false, name.clone(), None));
        lets.push(Stmt::Let(pat, None, Some(acg::path_expr(&name)), None));
    }
    if !lets.is_empty() {
        lets.append(&mut block.stmts);
        block.stmts = lets;
    }
    Ok(Some(ItemFn { attrs: item.attrs, sig, block }))
}

/// クロージャ式を関数にする (`fn call(__env: (), p0: (), ..) { 本体 }`: 型はシグネチャから決まる)
pub fn synth_closure(c: &Expr) -> ItemFn {
    let Expr::Closure(_, ps, _, body) = c else { unreachable!() };
    let unit = crate::parse::Ty::Tuple(vec![]);
    let mut params = vec![(Pat::Ident(false, false, "__env".into(), None), unit.clone())];
    let mut stmts = vec![];
    for (i, (p, _)) in ps.iter().enumerate() {
        let simple = matches!(p, Pat::Ident(false, _, _, None));
        let pin = if simple {
            p.clone()
        } else {
            let id = format!("__p{i}");
            stmts.push(Stmt::Let(p.clone(), None, Some(acg::path_expr(&id)), None));
            Pat::Ident(false, false, id, None)
        };
        params.push((pin, unit.clone()));
    }
    match &**body {
        Expr::Block(None, false, b) => stmts.extend(b.stmts.iter().cloned()),
        e => stmts.push(Stmt::Expr(e.clone(), false)),
    }
    let sig = crate::parse::FnSig {
        name: "call".into(),
        generics: crate::parse::Generics { params: vec![], preds: vec![] },
        recv: None,
        recv_ty: None,
        params,
        ret: None,
        quals: vec![],
    };
    ItemFn { attrs: vec![], sig, block: Block { stmts } }
}

/// 関数 (インスタンスも) id の本体
fn fn_ast(a: &Analysis, body: u32) -> R<ItemFn> {
    let ii = a.prog.item_of[body as usize].ok_or("internal: no item")?;
    let it = &a.items[ii as usize];
    if let Some(cd) = a.prog.closures.get(&body) {
        let mut encl = a.encl.lock().unwrap();
        if !encl.contains_key(&ii) {
            let f = parse_fn(&a.src, it)?.ok_or_else(|| format!("`{}` has no body", a.prog.names[body as usize]))?;
            encl.insert(ii, crate::ast_closure::closures_of(&f.block));
        }
        let cs = encl.get(&ii).unwrap();
        let c = cs.get(cd.k as usize).ok_or("internal: closure not found")?;
        return Ok(synth_closure(c));
    }
    let f = parse_fn(&a.src, it)?.ok_or_else(|| format!("`{}` has no body", a.prog.names[body as usize]))?;
    Ok(f)
}

/// 展開してよい関数: prelude の (クロージャでも特別な本体でもない) 小さい関数
fn inline_bodies(a: &Analysis) -> acg::Inline {
    let p = &a.prog;
    let mut inl = acg::Inline::default();
    let special = [
        crate::prelude::DROP_GLUE, "__exr_hash", "__exr_cmp", "__exr_eq", "__exr_clone", "__exr_default", "__exr_display", "__exr_debug",
        "__exr_rt_init", "__exr_on_trap", "Option::unwrap", "Option::expect", "Result::unwrap", "Result::expect", "Result::unwrap_err", "Result::expect_err",
    ];
    for id in 0..p.names.len() {
        let Some(ii) = p.item_of[id] else { continue };
        // (依存するクレートの関数は展開しない: 本体はインターフェースのハッシュに入らないので、変わっても気付けない)
        if a.ext_items.get(ii as usize).copied().unwrap_or(false) || !p.sigs[id].has_body || p.sigs[id].is_extern {
            continue;
        }
        // (クロージャの本体: 大きさは本体の構文木で見る (アイテムは囲む関数のもの))
        if p.closures.contains_key(&(id as u32)) {
            if std::env::var_os("CRUSTC_NO_INLINE_CLOSURE").is_some() {
                continue;
            }
            if let Ok(f) = fn_ast(a, id as u32) {
                let ok = acg::inline_candidate(&f) && format!("{:?}", f.block).len() <= 2400;
                if std::env::var_os("CRUSTC_INLINE_LOG").is_some() {
                    eprintln!("inline {} closure {}", if ok { "yes" } else { "no " }, p.names[id]);
                }
                if ok {
                    inl.bodies.insert(id as u32, f);
                    let it = &a.items[ii as usize];
                    if ii < p.prelude_from {
                        inl.user_lines.insert(id as u32, it.line as u32 - 1);
                    }
                }
            }
            continue;
        }
        if special.contains(&p.names[id].as_str()) {
            continue;
        }
        let it = &a.items[ii as usize];
        if it.end - it.start > 4000 {
            continue;
        }
        // (大きさはソースのトークンの数。式の Debug の長さは位置などで膨らむ)
        let ntok = lexer::tokenize(a.src[it.start..it.end].as_bytes()).len();
        if ntok > acg::MAX_TOKENS {
            continue;
        }
        if let Ok(f) = fn_ast(a, id as u32) {
            let ok = acg::inline_candidate(&f);
            if std::env::var_os("CRUSTC_INLINE_LOG").is_some() {
                eprintln!("inline {} {ntok} {}", if ok { "yes" } else { "no " }, p.names[id]);
            }
            if ok {
                inl.bodies.insert(id as u32, f);
                if ii < p.prelude_from {
                    inl.user_lines.insert(id as u32, it.line as u32 - 1);
                }
            }
        }
    }
    inl
}

fn concrete(p: &Program, id: usize) -> bool {
    p.item_of[id].is_some() && !p.sigs[id].is_generic() && p.sigs[id].has_body
}

/// 子プロセスから受け取るコンパイル結果の並び: (失敗か, 番号, 中身)
fn read_all(fd: i32) -> Vec<u8> {
    let mut out = vec![];
    let mut buf = vec![0u8; 1 << 16];
    loop {
        // SAFETY: buf は buf.len() byte ある
        let n = unsafe { c_read(fd, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    out
}

fn write_all(fd: i32, mut d: &[u8]) {
    while !d.is_empty() {
        // SAFETY: d は d.len() byte ある
        let n = unsafe { c_write(fd, d.as_ptr(), d.len()) };
        if n <= 0 {
            return;
        }
        d = &d[n as usize..];
    }
}

impl Session2 {
    /// 関数 id のキャッシュのキー: 本体のハッシュ + 自分と参照先のシグネチャ + 名前で現れる struct (レイアウトごと) + const

    /// closed: prelude の関数で、ユーザの関数を呼びえないもの (総称でないか、型引数が prelude の型だけ)。参照の
    /// シグネチャは prelude のものだけを入れる (名前の最後で多めに取ると、ユーザの `Acc::push` などが入って、同じ prelude の
    /// 関数がクレートごとに違うキーになり、どのクレートでもコンパイルし直していた)
    fn fn_key_r(&self, a: &Analysis, id: usize, closed: Option<&[bool]>) -> Key {
        let ii = a.prog.item_of[id].unwrap() as usize;
        let pf = a.prog.prelude_from as usize;
        // 閉じた依存するクレートの関数 (と、型引数に依存するクレートの型を持つ prelude の関数): 本体・自分のシグネチャと、
        // 見えるクレートの中の参照先だけ (prelude はコンパイラで決まる、ほかのクレートのものは呼べない)
        if let Some(u) = closed.filter(|u| ii < pf || u.iter().any(|&x| x)) {
            let in_u = |item: Option<u32>| item.is_some_and(|ri| a.crate_of_item(ri as usize).is_some_and(|c| u[c]));
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(&3u8);
            h.add(&(self.opts.opt as u8));
            h.add(&(self.opts.backend_id()));
            h.add(&a.items[ii].hash);
            h.add(&a.sig_key(id as u32));
            for &r in &a.refs[id] {
                if in_u(a.prog.item_of[r as usize]) {
                    h.add(&a.sig_key(r));
                }
            }
            for &st in &a.srefs[id] {
                let b = a.prog.base_of(st);
                if in_u(a.prog.struct_item.get(b as usize).copied()) {
                    h.add(&a.prog.structs[st as usize].deep);
                }
            }
            for &c in &a.crefs[id] {
                let cd = &a.prog.consts[c as usize];
                if in_u(Some(cd.item)) {
                    h.add(&cd.name);
                    h.add(&a.items[cd.item as usize].hash);
                    a.prog.hash_ty(cd.ty, &mut h);
                }
            }
            match a.prog.panic_handler {
                Some(p) => h.add(&a.prog.names[p as usize]),
                None => h.add(&0u8),
            }
            return h.finish();
        }
        let closed = closed.is_some();
        // 閉じた prelude の関数: prelude の本体とシグネチャはコンパイラ (cid) に入っているので、本体と自分のシグネチャと
        // panic の行き先 (`#[panic_handler]`) だけで決まる (参照先を名前の最後で多めに取ると、クレートごとに選ぶ prelude が
        // 違うのでキーが変わり、同じ関数をクレートごとにコンパイルし直していた)
        if closed && ii >= pf {
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(&2u8);
            h.add(&(self.opts.opt as u8));
            h.add(&(self.opts.backend_id()));
            h.add(&a.items[ii].hash);
            h.add(&a.sig_key(id as u32));
            match a.prog.panic_handler {
                Some(p) => h.add(&a.prog.names[p as usize]),
                None => h.add(&0u8),
            }
            return h.finish();
        }
        let mut h = KeyHasher::new();
        h.add(&self.cid);
        h.add(&1u8);
        h.add(&(self.opts.opt as u8));
        h.add(&(self.opts.backend_id()));
        h.add(&a.items[ii].hash);
        if a.prog.closures.contains_key(&(id as u32)) {
            h.add(&a.prog.names[id]);
        }
        h.add(&a.sig_key(id as u32));
        for &r in &a.refs[id] {
            if closed && a.prog.item_of[r as usize].is_some_and(|ri| (ri as usize) < pf) {
                continue;
            }
            h.add(&a.sig_key(r));
            // (展開しうるユーザの関数は、本体と行も効く)
            if let Some(&lb) = a.inl.user_lines.get(&r) {
                if let Some(ri) = a.prog.item_of[r as usize] {
                    h.add(&a.items[ri as usize].hash);
                    h.add(&lb);
                }
            }
        }
        for &st in &a.srefs[id] {
            if closed && !a.prog.is_prelude_ty(Ty::Struct(st)) {
                continue;
            }
            h.add(&a.prog.structs[st as usize].deep);
        }
        for &c in &a.crefs[id] {
            let cd = &a.prog.consts[c as usize];
            h.add(&cd.name);
            h.add(&a.items[cd.item as usize].hash);
            a.prog.hash_ty(cd.ty, &mut h);
        }
        h.finish()
    }

    /// 関数かインスタンスのキー (インスタンスは元の関数のキー + 型引数)
    fn key_of(&self, a: &Analysis, insts: &Mutex<Insts>, id: u32) -> Key {
        let g = insts.lock().unwrap();
        let body = g.body_of(id) as usize;
        let closed = a.closed_univ(&*g, id);
        let k = self.fn_key_r(a, body, closed.as_ref().map(|v| v.as_slice()));
        match g.get(id) {
            None => k,
            Some(inst) => {
                let mut h = KeyHasher::new();
                h.add(&k);
                h.add(&7u8);
                for &t in &inst.args {
                    a.prog.hash_ty(t, &mut h);
                }
                h.finish()
            }
        }
    }

}

/// compile_id と、コード生成が要求した (総称関数の) インスタンス
fn compile_id_req(a: &Analysis, insts: &Mutex<Insts>, id: u32) -> R<(Blob, Vec<u32>)> {
    let body = insts.lock().unwrap().body_of(id);
    // (本体の構文木は本体ごとに 1 回だけ読む: 総称関数のインスタンスごとに読み直していた (コンパイルの 10%))
    let mut asts = a.asts.lock().unwrap();
    if !asts.contains_key(&body) {
        let f = fn_ast(a, body)?;
        asts.insert(body, f);
    }
    let f = asts.get(&body).unwrap();
    let ii = a.prog.item_of[body as usize].unwrap() as usize;
    let it = &a.items[ii];
    // (acg のエラーはファイルの中の `行:桁:` を持つ。持たないものはアイテムの先頭の行にする)
    let lb = {
        let g = insts.lock().unwrap();
        a.line_base_for(&*g, id).unwrap_or(it.line as u32 - 1)
    };
    let (c, req) = acg::compile(&a.prog, insts, id, f, lb, &a.inl)
        .map_err(|e| if acg::has_loc(&e) { e } else { format!("{}:1: in `{}`: {e}", it.line, a.prog.names[body as usize]) })?;
    // (check は機械語にしない。ビルドは記録をバックエンドで機械語にする)
    if a.inl.check_only {
        return Ok((acg::Compiled::empty_blob(), req));
    }
    lower_blob(a, c, req)
}

/// 記録を機械語にして Blob を作る (バックエンドなしでは作れない)
#[cfg(not(feature = "build"))]
fn lower_blob(_: &Analysis, _: acg::Compiled, _: Vec<u32>) -> R<(Blob, Vec<u32>)> { Err("internal: crustc-core was built without the backend (feature `build`)".into()) }

/// 位置付きのエラー (行・桁は 1 始まり)。check / LSP が返す
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl Diagnostic {
    /// 内部の "L:C: msg" (か "L: msg") の形の文字列から作る
    pub fn parse(s: &str) -> Diagnostic {
        let b = s.as_bytes();
        let num = |from: usize| -> (usize, usize) {
            let mut i = from;
            while i < b.len() && b[i] == b' ' {
                i += 1;
            }
            let st = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            (s[st..i].parse().unwrap_or(0), i)
        };
        let (line, i) = num(0);
        if line > 0 && i < b.len() && b[i] == b':' {
            let (col, j) = num(i + 1);
            if col > 0 && j < b.len() && b[j] == b':' {
                return Diagnostic { line, col, msg: s[j + 1..].trim().to_string() };
            }
            return Diagnostic { line, col: 1, msg: s[i + 1..].trim().to_string() };
        }
        Diagnostic { line: 1, col: 1, msg: s.to_string() }
    }
}

impl Session2 {
    /// 型検査だけを行い、全診断を返す (`crustc check` と LSP)。到達しない関数も、総称関数は要求されたインスタンスごとに
    /// 検査する (コード生成までしてエラーを集め、結果はキャッシュに入れる: 次の検査・ビルドはそれを使う)。prelude の中の診断は出さない
    pub fn check(&mut self, src: &str) -> Vec<Diagnostic> {
        let user_lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
        let mut a = match self.analyze(src) {
            Ok(a) => a,
            Err(e) => return vec![Diagnostic::parse(&e)],
        };
        // 検査だけ: コード生成 (acg) で型を検査し、機械語は作らない (CRUSTC_CHECK_CODEGEN=1 で機械語まで作る)。
        // 検査済みの印は、要求したインスタンスへの再配置だけを持つ空の Blob を、ビルドとは別のキーで覚える
        // (ビルドの Blob があればそれも検査済みとして使う)
        let check_only = std::env::var_os("CRUSTC_CHECK_CODEGEN").is_none();
        a.inl.check_only = check_only;
        let mut tm = Timer { on: std::env::var_os("CRUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
        let (mut n_keys, mut n_hit, mut n_checked, mut n_memo) = (0, 0, 0, 0);
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut errs: Vec<String> = vec![];
        let mut done: FxSet<u32> = FxSet::default();
        // 前回の check の記録: 宣言の指紋が同じなら、本体のアイテムが変わっていない関数は検査済み (キーも読み戻しも要らない)。
        // 要求していたインスタンスは名前で辿り、本体が変わったものだけを要求し直す
        let old = match self.chk.take() {
            Some(m) if m.fp == a.iface_fp && check_only && std::env::var_os("CRUSTC_NO_CHECK_MEMO").is_none() => Some(m),
            _ => None,
        };
        // (記録を作るのは、続けて検査する (キャッシュを使う) セッションだけ: 1 度だけの `crustc check` では作らない)
        let keep = check_only && !self.opts.no_cache;
        let mut memo = ChkMemo { fp: a.iface_fp, ents: FxMap::default() };
        let mut seen: FxSet<String> = FxSet::default();
        let name_of = |g: &Insts, id: u32| -> String {
            match g.get(id) {
                Some(i) => format!("{}\u{1}{}", a.prog.names[i.base as usize], i.sig.symbol),
                None => a.prog.names[id as usize].clone(),
            }
        };
        let body_hash = |name: &str| -> Option<Key> {
            let &id = a.prog.index.get(name)?;
            let ii = a.prog.item_of[id as usize]?;
            Some(a.items[ii as usize].hash)
        };
        let n = a.prog.names.len();
        let mut wave: Vec<u32> = (0..n)
            .filter(|&i| concrete(&a.prog, i) && a.prog.item_of[i].is_some_and(|ii| ii < a.prog.prelude_from) && !a.is_ext(i as u32))
            .map(|i| i as u32)
            .collect();
        while !wave.is_empty() {
            for &id in &wave {
                if !done.insert(id) {
                    continue;
                }
                let (name, body) = {
                    let g = insts.lock().unwrap();
                    (name_of(&*g, id), a.prog.names[g.body_of(id) as usize].clone())
                };
                if !seen.insert(name.clone()) {
                    continue;
                }
                // (記録から: 本体が同じなら、要求していたインスタンスを名前で辿る)
                if let Some(o) = &old {
                    if let Some(e) = o.ents.get(&name) {
                        if e.ok && Some(e.body_hash) == body_hash(&e.body) {
                            n_memo += 1;
                            let mut st: Vec<String> = e.children.clone();
                            memo.ents.insert(name.clone(), e.clone());
                            while let Some(c) = st.pop() {
                                if seen.contains(&c) {
                                    continue;
                                }
                                let Some(ce) = o.ents.get(&c) else { continue };
                                if ce.ok && Some(ce.body_hash) == body_hash(&ce.body) {
                                    seen.insert(c.clone());
                                    n_memo += 1;
                                    st.extend(ce.children.iter().cloned());
                                    memo.ents.insert(c, ce.clone());
                                } else {
                                    // (本体が変わったインスタンス: 要求し直して検査する)
                                    let _ = crate::cache2::decode_blob(&ce.me, &a.prog, &insts);
                                }
                            }
                            continue;
                        }
                    }
                }
                let k = self.key_of(&a, &insts, id);
                let kc = k ^ CHECK_SALT;
                n_keys += 1;
                // (キャッシュにあれば検査済み: 読み戻すと要求していたインスタンスも要求し直す)
                let hit = match self.get(k).and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)) {
                    Some(b) => Some(b),
                    None if check_only => self.get(kc).and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)),
                    None => None,
                };
                let mut ok = true;
                let req: Option<Vec<u32>> = match hit {
                    Some(b) => {
                        n_hit += 1;
                        Some(b.relocs.iter().filter_map(|r| match r.target { acg::Target::Func(i) => Some(i), _ => None }).collect())
                    }
                    None => {
                        n_checked += 1;
                        match compile_id_req(&a, &insts, id) {
                            Ok((b, req)) => {
                                if !self.opts.no_cache {
                                    let b = if check_only {
                                        let relocs = req.iter().map(|&i| acg::BlobReloc { offset: 0, kind: 4, addend: 0, target: acg::Target::Func(i) }).collect();
                                        Blob { relocs, ..b }
                                    } else {
                                        b
                                    };
                                    let mut e = vec![];
                                    {
                                        let g = insts.lock().unwrap();
                                        crate::cache2::encode_blob(&b, &a.prog, &*g, &mut e);
                                    }
                                    self.put(if check_only { kc } else { k }, e);
                                }
                                Some(req)
                            }
                            Err(e) => {
                                errs.push(e);
                                ok = false;
                                Some(vec![])
                            }
                        }
                    }
                };
                // (記録)
                if !keep {
                    continue;
                }
                if let (Some(req), Some(bh)) = (req, body_hash(&body)) {
                    let g = insts.lock().unwrap();
                    let children: Vec<String> = req.iter().filter(|&&i| g.get(i).is_some()).map(|&i| name_of(&*g, i)).collect();
                    let mut me = vec![];
                    if g.get(id).is_some() {
                        let b = Blob { align: 1, code: vec![], relocs: vec![acg::BlobReloc { offset: 0, kind: 4, addend: 0, target: acg::Target::Func(id) }], datas: vec![], vtables: vec![], traps: vec![], line_base: None, code_ref: None };
                        crate::cache2::encode_blob(&b, &a.prog, &*g, &mut me);
                    }
                    memo.ents.insert(name, ChkEnt { body, body_hash: bh, children, me, ok });
                }
            }
            wave = std::mem::take(&mut insts.lock().unwrap().pending);
            wave.retain(|id| !done.contains(id));
        }
        if keep {
            self.chk = Some(memo);
        }
        tm.lap("check", format!("{n_memo} from the last check, {n_keys} keys, {n_hit} cached, {n_checked} checked"));
        let mut d: Vec<Diagnostic> = errs.iter().map(|e| Diagnostic::parse(e)).filter(|d| d.line <= user_lines).collect();
        d.sort_by(|x, y| (x.line, x.col).cmp(&(y.line, y.col)));
        d.dedup();
        d
    }
}
