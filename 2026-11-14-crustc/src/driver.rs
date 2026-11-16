//! コンパイルのセッション。CLI・LSP・ベンチが同じ経路を通る。
//!
//! パイプライン:
//!   lex (自前) → 変わったアイテムだけ syn で要約 → 到達可能性 → 関数ごとのキャッシュキー
//!   → ミスした関数だけ parse + 型検査 + Cranelift → 1 個の .o に詰める → リンク (変化がなければ省略)
//!
//! キャッシュは「ディスク上のファイル (読み込み専用)」+「このセッションで作ったエントリ (メモリ)」
//! の 2 層。CLI はビルドの最後に、今回使ったエントリだけをファイルへ書き戻す。LSP はメモリだけを使い、
//! 毎回の check の後に、今回使わなかったエントリを捨てる (メモリ使用量はコード量で抑えられる)。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::{Linkage, Module, ModuleReloc, ModuleRelocTarget};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::cache::{self, Cache, Key, KeyHasher};
use crate::codegen::{self, Blob, RELOC_KINDS, Target, Worker};
use std::sync::Mutex;

use crate::front::{self, Insts, Program, Summary};
use crate::fxhash::{FxMap, FxSet};
use crate::lexer::{self, ItemKind, SrcItem};
use crate::link;

#[derive(Clone, Debug)]
pub struct Options {
    pub opt: bool,
    pub fast_regalloc: bool,
    pub linker: Option<String>,
    pub time: bool,
    pub jobs: usize,
    pub no_cache: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            opt: false,
            fast_regalloc: true,
            linker: None,
            time: false,
            jobs: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            no_cache: false,
        }
    }
}

/// 位置付きのエラー (行・桁は 1 始まり)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl Diagnostic {
    /// 内部の "L:C: msg" 形式の文字列から作る
    pub fn parse(s: &str) -> Diagnostic {
        let mut it = s.splitn(3, ':');
        if let (Some(l), Some(c), Some(m)) = (it.next(), it.next(), it.next()) {
            if let (Ok(line), Ok(col)) = (l.trim().parse(), c.trim().parse()) {
                return Diagnostic {
                    line,
                    col,
                    msg: m.trim().to_string(),
                };
            }
        }
        if let Some((l, m)) = s.split_once(':') {
            if let Ok(line) = l.trim().parse() {
                return Diagnostic {
                    line,
                    col: 1,
                    msg: m.trim().to_string(),
                };
            }
        }
        Diagnostic {
            line: 1,
            col: 1,
            msg: s.to_string(),
        }
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

#[derive(Clone, Debug, Default)]
pub struct BuildReport {
    pub fns_live: usize,
    pub fns_compiled: usize,
    pub items_parsed: usize,
    pub object_bytes: usize,
    pub linked: bool,
    pub binary_bytes: u64,
    pub cache_bytes: usize,
}

pub struct Session {
    pub opts: Options,
    isa: OwnedTargetIsa,
    cid: (u64, u128),
    disk: Cache,
    mem: FxMap<Key, Vec<u8>>,
    used: FxSet<Key>,
    /// prelude の選択の、アイテムごとの結果
    sel: crate::prelude::SelectCache,
}

struct Timer {
    on: bool,
    start: Instant,
    last: Instant,
}
impl Timer {
    fn lap(&mut self, what: &str, extra: impl std::fmt::Display) {
        if self.on {
            let now = Instant::now();
            eprintln!(
                "  {:<10} {:>8.2} ms  {}",
                what,
                (now - self.last).as_secs_f64() * 1e3,
                extra
            );
            self.last = now;
        }
    }
}

/// 前回の出力の `.o` の関数の機械語 (関数ごとのセクション) を、(長さ, ハッシュ) で引く
struct ObjCode {
    data: Vec<u8>,
    index: FxMap<(u32, u64), (usize, usize)>,
}

impl ObjCode {
    fn load(path: &Path) -> ObjCode {
        let data = std::fs::read(path).unwrap_or_default();
        let mut index = FxMap::default();
        let rd = |d: &[u8], o: usize, n: usize| -> Option<u64> {
            let b = d.get(o..o + n)?;
            let mut v = [0u8; 8];
            v[..n].copy_from_slice(b);
            Some(u64::from_le_bytes(v))
        };
        // ELF64 の symtab を辿り、実行可能なセクションにある関数のシンボル (位置と大きさ) の機械語を全部ハッシュする
        // (`.text` が圧縮されていれば解凍してから。data は解凍した .text をつないだもの)
        let raw = data;
        let mut data = vec![];
        (|| -> Option<()> {
            let secs = crate::elfz::sections(&raw)?;
            let mut text_at: FxMap<usize, usize> = FxMap::default();
            for (i, s) in secs.iter().enumerate() {
                if s.ty != 2 || s.entsize != 24 {
                    continue; // SHT_SYMTAB
                }
                for k in 0..s.size / 24 {
                    let e = s.off + k * 24;
                    let info = *raw.get(e + 4)?;
                    let shndx = rd(&raw, e + 6, 2)? as usize;
                    let (value, sz) = (rd(&raw, e + 8, 8)? as usize, rd(&raw, e + 16, 8)? as usize);
                    if info & 0xF != 2 || sz == 0 || shndx == 0 || shndx >= secs.len() {
                        continue; // STT_FUNC で、定義されているもの
                    }
                    let ts = &secs[shndx];
                    if ts.ty != 1 || ts.flags & 4 == 0 {
                        continue;
                    }
                    let base = match text_at.get(&shndx) {
                        Some(&b) => b,
                        None => {
                            let b = data.len();
                            data.extend_from_slice(&crate::elfz::section_bytes(&raw, ts)?);
                            text_at.insert(shndx, b);
                            b
                        }
                    };
                    let code = data.get(base + value..base + value + sz)?;
                    index.insert((sz as u32, crate::cache::code_hash(code)), (base + value, sz));
                }
                let _ = i;
            }
            Some(())
        })();
        ObjCode { data, index }
    }

    fn find(&self, len: u32, h: u64) -> Option<&[u8]> {
        let &(o, n) = self.index.get(&(len, h))?;
        Some(&self.data[o..o + n])
    }
}

fn make_isa(o: &Options) -> OwnedTargetIsa {
    let mut f = settings::builder();
    f.set("opt_level", if o.opt { "speed_and_size" } else { "none" })
        .unwrap();
    f.set(
        "regalloc_algorithm",
        if o.fast_regalloc {
            "single_pass"
        } else {
            "backtracking"
        },
    )
    .unwrap();
    f.set("is_pic", "true").unwrap();
    // 開発ビルドではアンウィンド情報不要 (panic/unwind をまだ持たない)
    f.set("unwind_info", "false").unwrap();
    // i128 の引数と戻り値 (SysV と同じく 2 レジスタ)
    f.set("enable_llvm_abi_extensions", "true").unwrap();
    cranelift_native::builder()
        .unwrap()
        .finish(settings::Flags::new(f))
        .unwrap()
}

/// コンパイラ自身の同一性。実行ファイルのサイズと mtime を使う
/// (コンパイラを作り直したらキャッシュは全て無効になる。安全側)。
fn compiler_id() -> (u64, u128) {
    let exe = std::env::current_exe().ok();
    let md = exe.and_then(|p| std::fs::metadata(p).ok());
    let len = md.as_ref().map(|m| m.len()).unwrap_or(0);
    let mt = md
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (len, mt)
}

pub fn cache_path(output: &Path) -> PathBuf {
    let mut s = output.as_os_str().to_owned();
    s.push(".crcache");
    PathBuf::from(s)
}

const KEEP_PARSED: usize = 32;

/// フロントエンドの結果 (check と build で共通)
struct Analysis {
    items: Vec<SrcItem>,
    sums: Vec<Summary>,
    sum_keys: Vec<Key>,
    prog: Program,
    refs: Vec<Vec<u32>>,
    srefs: Vec<Vec<u32>>,
    parsed: HashMap<usize, syn::Item>,
    n_parsed: usize,
    /// アイテム単位のエラー (そのアイテムは空の要約として扱う)
    errors: Vec<String>,
    /// 関数ごとの const / static の参照
    crefs: Vec<Vec<u32>>,
}

impl Session {
    pub fn new(opts: Options) -> Session {
        let isa = make_isa(&opts);
        Session {
            opts,
            isa,
            cid: compiler_id(),
            disk: Cache::empty(),
            mem: FxMap::default(),
            used: FxSet::default(),
            sel: Default::default(),
        }
    }

    /// ディスク上のキャッシュを読み込む (以後の参照で使われる)
    pub fn load_cache(&mut self, path: &Path) {
        if !self.opts.no_cache {
            self.disk = Cache::open(path);
        }
    }

    /// メモリ上のエントリ数とバイト数
    pub fn mem_usage(&self) -> (usize, usize) {
        (self.mem.len(), self.mem.values().map(|v| v.len()).sum())
    }

    fn get(&mut self, k: Key) -> Option<&[u8]> {
        self.used.insert(k);
        match self.mem.get(&k) {
            Some(v) => Some(v),
            None => self.disk.get(k),
        }
    }

    fn put(&mut self, k: Key, v: Vec<u8>) {
        self.used.insert(k);
        self.mem.insert(k, v);
    }

    /// 今回使わなかったメモリ上のエントリを捨てる
    fn gc(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.mem.retain(|k, _| used.contains(k));
    }

    fn analyze(&mut self, src: &str, items: Vec<SrcItem>) -> Result<Analysis, String> {
        let cid = self.cid;
        let sum_keys: Vec<Key> = items
            .iter()
            .map(|it| {
                let mut h = KeyHasher::new();
                h.add(&(cid, 0u8, it.hash));
                h.finish()
            })
            .collect();
        let mut sums: Vec<Option<Summary>> = sum_keys
            .iter()
            .map(|&k| self.get(k).and_then(Summary::decode))
            .collect();
        let todo: Vec<usize> = (0..items.len()).filter(|&i| sums[i].is_none()).collect();
        let n_parsed = todo.len();
        let mut errors = vec![];
        let mut failed = HashSet::new();
        let mut record =
            |ii: usize, r: Result<Summary, String>, sums: &mut Vec<Option<Summary>>| match r {
                Ok(sm) => sums[ii] = Some(sm),
                Err(e) => {
                    errors.push(e);
                    failed.insert(ii);
                    sums[ii] = Some(Summary::default());
                }
            };
        // impl / trait ヘッダを先に要約する (メンバの `Self` / trait 名を決めるのに必要)
        let is_header = |k: ItemKind| matches!(k, ItemKind::ImplHeader | ItemKind::TraitHeader);
        for &ii in todo.iter().filter(|&&i| is_header(items[i].kind)) {
            let r = front::parse_and_summarize(src, &items[ii], None).map(|x| x.0);
            record(ii, r, &mut sums);
        }
        // メンバの所有者 (impl なら self 型と型パラメータ、trait なら trait 名)
        let owners: Vec<Option<front::Owner>> = items
            .iter()
            .map(|it| match it.kind {
                ItemKind::ImplMember(h) => {
                    let hs = sums[h as usize].as_ref().unwrap();
                    hs.impl_self.clone().map(|n| front::Owner {
                        self_ty: hs
                            .impl_self_ty
                            .clone()
                            .unwrap_or(front::TySyn::Named(n.clone())),
                        base: n,
                        impl_trait: hs.impl_trait.clone(),
                        generics: hs.impl_generics.clone(),
                        is_trait: false,
                    })
                }
                ItemKind::TraitMember(h) => sums[h as usize]
                    .as_ref()
                    .unwrap()
                    .trait_decl
                    .clone()
                    .map(|n| front::Owner {
                        self_ty: front::TySyn::Named("Self".into()),
                        generics: vec![("Self".into(), vec![n.clone()])],
                        base: n,
                        impl_trait: None,
                        is_trait: true,
                    }),
                _ => None,
            })
            .collect();
        let owner = |ii: usize| owners[ii].as_ref();
        let todo: Vec<usize> = todo.into_iter().filter(|&i| sums[i].is_none()).collect();
        let mut parsed: HashMap<usize, syn::Item> = HashMap::new();
        if todo.len() <= KEEP_PARSED {
            // インクリメンタル: その場で parse し、AST は codegen 用に取っておく
            for &ii in &todo {
                match front::parse_and_summarize(src, &items[ii], owner(ii)) {
                    Ok((sm, item)) => {
                        if let Some(item) = item {
                            parsed.insert(ii, item);
                        }
                        record(ii, Ok(sm), &mut sums);
                    }
                    Err(e) => record(ii, Err(e), &mut sums),
                }
            }
        } else {
            // cold: 並列に parse して要約だけ持ち帰る (AST は !Send なのでスレッド内で捨てる)
            let done = par_map(self.opts.jobs, &todo, 32, |ii| {
                Ok(front::parse_and_summarize(src, &items[ii], owner(ii)).map(|x| x.0))
            })?;
            for (ii, r) in todo.iter().zip(done) {
                record(*ii, r, &mut sums);
            }
        }
        let sums: Vec<Summary> = sums.into_iter().map(Option::unwrap).collect();
        // 成功した要約だけキャッシュに入れる
        for (ii, (k, sm)) in sum_keys.iter().zip(&sums).enumerate() {
            if !failed.contains(&ii) {
                if self.mem.contains_key(k) || self.disk.get(*k).is_some() {
                    self.used.insert(*k);
                } else {
                    let mut b = vec![];
                    sm.encode(&mut b);
                    self.put(*k, b);
                }
            }
        }
        let mut prog = Program::build(&items, &sums)?;
        prog.prelude_from = self.sel.user_items as u32;
        prog.overflow_checks = !self.opts.opt;
        prog.opt = self.opts.opt;
        prog.resolve_opaque(src, &items)?;
        let mut refs = Vec::with_capacity(prog.item_of.len());
        let mut srefs = Vec::with_capacity(prog.item_of.len());
        let mut crefs = Vec::with_capacity(prog.item_of.len());
        for ii in &prog.item_of {
            let (f, st, c) = ii
                .map(|ii| prog.refs(src, &items[ii as usize]))
                .unwrap_or_default();
            refs.push(f);
            srefs.push(st);
            crefs.push(c);
        }
        Ok(Analysis {
            items,
            sums,
            sum_keys,
            prog,
            refs,
            srefs,
            crefs,
            parsed,
            n_parsed,
            errors,
        })
    }

    /// 関数 id の内容キー: 本体ハッシュ + 自分と参照先のシグネチャ + 名前で現れる struct
    fn fn_key(a: &Analysis, id: usize, base: Key) -> Key {
        let ii = a.prog.item_of[id].unwrap() as usize;
        let mut h = KeyHasher::new();
        h.add(&(base, a.items[ii].hash));
        // 同じアイテムから複数の関数ができる (クロージャ) ので名前も入れる
        if a.prog.closures.contains_key(&(id as u32)) {
            h.add(&a.prog.names[id]);
        }
        // 自分のシグネチャ (struct 引数のレイアウトを含む) と、
        // 参照先はシグネチャだけ (本体の変更は呼び出し元に波及しない)
        a.prog.hash_sig(id as u32, &mut h);
        for &r in &a.refs[id] {
            a.prog.hash_sig(r, &mut h);
        }
        // 名前で現れる struct はレイアウトごと (deep hash)
        for &s in &a.srefs[id] {
            h.add(&a.prog.structs[s as usize].deep);
        }
        // const は式ごと埋め込むので定義のハッシュ、static は名前と型
        for &c in &a.crefs[id] {
            let cd = &a.prog.consts[c as usize];
            h.add(&(cd.name.as_str(), a.items[cd.item as usize].hash));
            a.prog.hash_ty(cd.ty, &mut h);
        }
        h.finish()
    }

    /// 関数かインスタンスのキー。インスタンスは元の関数のキー + 型引数 (struct は deep hash)
    fn key_of(a: &Analysis, insts: &Mutex<Insts>, id: u32, base: Key) -> Key {
        let g = insts.lock().unwrap();
        let k = Self::fn_key(a, g.body_of(id) as usize, base);
        match g.get(id) {
            None => k,
            Some(inst) => {
                let mut h = KeyHasher::new();
                h.add(&(k, 7u8));
                for &t in &inst.args {
                    a.prog.hash_ty(t, &mut h);
                }
                h.finish()
            }
        }
    }

    /// 直接コンパイルする関数か (ジェネリック関数・本体のない trait 宣言は除く)
    fn concrete(p: &Program, id: usize) -> bool {
        p.item_of[id].is_some() && !p.sigs[id].is_generic() && p.sigs[id].has_body
    }

    /// 型検査だけを行い、全診断を返す (LSP 用)。到達しない関数も検査する。
    /// 検査に通った関数は「OK」の印をキャッシュし、次回は parse もしない。
    pub fn check(&mut self, src: &str) -> Vec<Diagnostic> {
        let out = match with_prelude(src, &mut self.sel) {
            Ok((full, items)) => self.check_inner(&full, items),
            Err(e) => vec![e],
        };
        // prelude の中の診断は出さない (ユーザのコードではない)
        let out: Vec<String> = if out.is_empty() {
            out
        } else {
            let user_lines = src.lines().count() + 1;
            out.into_iter()
                .filter(|e| Diagnostic::parse(e).line <= user_lines)
                .collect()
        };
        self.gc();
        let mut d: Vec<Diagnostic> = out.iter().map(|e| Diagnostic::parse(e)).collect();
        d.sort_by_key(|d| (d.line, d.col));
        d.dedup();
        d
    }

    fn check_inner(&mut self, src: &str, items: Vec<SrcItem>) -> Vec<String> {
        let mut a = match self.analyze(src, items) {
            Ok(a) => a,
            Err(e) => return vec![e],
        };
        let mut errs = std::mem::take(&mut a.errors);
        let mut base = KeyHasher::new();
        base.add(&(self.cid, 2u8));
        let base = base.finish();
        // 検査済みの印がない関数だけ検査する。ジェネリック関数は、要求されたインスタンスごとに検査する
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut ids: Vec<u32> = (0..a.prog.names.len())
            .filter(|&i| Self::concrete(&a.prog, i))
            .map(|i| i as u32)
            .collect();
        let mut parsed = std::mem::take(&mut a.parsed);
        while !ids.is_empty() {
            let mut todo = vec![];
            for &id in &ids {
                let k = Self::key_of(&a, &insts, id, base);
                // 検査済みなら、その関数が要求するインスタンスだけ要求し直す
                let hit = match self.get(k) {
                    Some(d) => cache::request_insts(d, &a.prog, &insts),
                    None => false,
                };
                if !hit {
                    todo.push((id, k));
                }
            }
            let tids: Vec<u32> = todo.iter().map(|&(id, _)| id).collect();
            let results = check_all(
                &self.opts,
                src,
                &a.items,
                &a.prog,
                &insts,
                &tids,
                std::mem::take(&mut parsed),
                &self.isa,
            );
            for ((_, k), r) in todo.into_iter().zip(results) {
                match r {
                    Ok(req) => {
                        let mut b = vec![];
                        cache::encode_insts(&req, &a.prog, &insts.lock().unwrap(), &mut b);
                        self.put(k, b);
                    }
                    Err(e) => errs.push(e),
                }
            }
            ids = std::mem::take(&mut insts.lock().unwrap().pending);
        }
        errs
    }

    /// ビルドして実行ファイルを書く。キャッシュの書き戻しは save_cache で行う。
    pub fn build(&mut self, src: &str, output: &Path) -> Result<BuildReport, String> {
        let r = with_prelude(src, &mut self.sel).and_then(|(full, items)| self.build_inner(&full, items, output));
        if r.is_err() {
            self.used.clear();
        }
        r
    }

    fn build_inner(
        &mut self,
        src: &str,
        items: Vec<SrcItem>,
        output: &Path,
    ) -> Result<BuildReport, String> {
        let t0 = Instant::now();
        let mut tm = Timer {
            on: self.opts.time,
            start: t0,
            last: t0,
        };
        let mut a = self.analyze(src, items)?;
        if let Some(e) = a.errors.first() {
            return Err(e.clone());
        }
        tm.lap(
            "front",
            format!("{} items, {} parsed", a.items.len(), a.n_parsed),
        );

        let mut live = a.prog.reachable(&a.refs);
        let mut base = KeyHasher::new();
        base.add(&(self.cid, 1u8));
        base.add(self.isa.triple().to_string().as_str());
        base.add(&(self.opts.opt, self.opts.fast_regalloc));
        let base = base.finish();
        let mut user_live: Vec<u32> = (0..a.prog.names.len())
            .filter(|&i| live[i] && Self::concrete(&a.prog, i))
            .map(|i| i as u32)
            .collect();
        let n_user = a.prog.item_of.iter().filter(|i| i.is_some()).count();
        tm.lap(
            "plan",
            format!(
                "{} live fns, {} unreachable skipped",
                user_live.len(),
                n_user - user_live.len()
            ),
        );

        // 関数 → (キャッシュ or コンパイル)。その途中で要求されたインスタンスを、不動点まで同じように処理する
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut blobs: FxMap<u32, Blob> = FxMap::default();
        let mut parsed = std::mem::take(&mut a.parsed);
        let mut n_hit = 0;
        let mut n_compiled = 0;
        let mut wave = user_live.clone();
        let mut old_code: Option<ObjCode> = None;
        // 今回コンパイルした関数 (`.o` を作ってからキャッシュに書く)
        let mut fresh: Vec<(u32, Key)> = vec![];
        while !wave.is_empty() {
            let mut misses = vec![];
            let mut keys: FxMap<u32, Key> = FxMap::default();
            for &id in &wave {
                let k = Self::key_of(&a, &insts, id, base);
                keys.insert(id, k);
                // (場所を書き込んだ関数は、アイテムの開始行が変わっていたら使わない)
                let lb = {
                    let body = insts.lock().unwrap().body_of(id);
                    a.prog.item_of[body as usize].map(|i| a.items[i as usize].line as u32 - 1)
                };
                let hit = self
                    .get(k)
                    .and_then(|d| cache::decode_blob(d, &a.prog, &insts))
                    .filter(|b| b.line_base.is_none() || b.line_base == lb);
                // (機械語の中身は前回の `.o` から。見つからなければ (`.o` を消した、など) コンパイルし直す)
                let hit = hit.and_then(|mut b| {
                    if let Some((len, h)) = b.code_ref {
                        let oc = old_code.get_or_insert_with(|| ObjCode::load(&output.with_extension("o")));
                        b.code = oc.find(len, h)?.to_vec();
                        b.code_ref = None;
                    }
                    Some(b)
                });
                match hit {
                    Some(b) => {
                        n_hit += 1;
                        blobs.insert(id, b);
                    }
                    None => misses.push(id),
                }
            }
            let compiled = compile_all(
                &self.opts,
                src,
                &a.items,
                &a.prog,
                &insts,
                &misses,
                std::mem::take(&mut parsed),
                &self.isa,
            )?;
            n_compiled += misses.len();
            for (id, b) in compiled {
                fresh.push((id, keys[&id]));
                blobs.insert(id, b);
            }
            wave = std::mem::take(&mut insts.lock().unwrap().pending);
            wave.retain(|id| !blobs.contains_key(id));
            // codegen の途中で作ったインスタンス (書式化・drop glue など) の本体が名前で参照する関数も
            // 到達可能にする (main からの参照グラフにはなかったもの)
            let starts: Vec<u32> = {
                let g = insts.lock().unwrap();
                wave.iter().map(|&id| g.body_of(id)).collect()
            };
            let mut stack: Vec<u32> = starts
                .iter()
                .flat_map(|&b| a.refs[b as usize].iter().copied())
                .collect();
            // 書式化の glue は Display / Debug の impl を名前を書かずに呼ぶ
            if starts.iter().any(|&b| {
                matches!(a.prog.names[b as usize].as_str(), "__exr_display" | "__exr_debug")
            }) {
                stack.extend((0..a.prog.names.len() as u32).filter(|&i| {
                    matches!(a.prog.sigs[i as usize].impl_trait.as_deref(), Some("Display" | "Debug"))
                }));
            }
            while let Some(i) = stack.pop() {
                if std::mem::replace(&mut live[i as usize], true) {
                    continue;
                }
                stack.extend(a.refs[i as usize].iter().copied());
                if Self::concrete(&a.prog, i as usize) && !blobs.contains_key(&i) {
                    user_live.push(i);
                    wave.push(i);
                }
            }
        }
        let insts = insts.into_inner().unwrap();
        tm.lap(
            "codegen",
            format!(
                "{n_hit} hit / {n_compiled} compiled ({} generic instances)",
                insts.list.len()
            ),
        );

        let (obj, placed) = emit_object(&a.prog, &insts, &live, &user_live, &blobs, self.isa.clone())?;
        // 機械語が `.o` に入った関数は (長さ, ハッシュ) だけ、入らなかった (どこからも呼ばれない) 関数は機械語ごと書く
        {
            let g = &insts;
            for (id, k) in fresh {
                let mut buf = vec![];
                cache::encode_blob(&blobs[&id], &a.prog, g, &mut buf, placed.contains(&id));
                self.put(k, buf);
            }
        }
        let mut oh = KeyHasher::new();
        oh.add(&obj[..]);
        oh.add(&self.opts.linker); // リンカを変えたら再リンク
        let obj_key = oh.finish();
        tm.lap("object", format!("{} bytes", obj.len()));

        // リンク (オブジェクトが前回と同一なら省略)。前回の obj キーは「obj → 出力」の対応として保存
        let mut lk = KeyHasher::new();
        lk.add(&(3u8, output.to_string_lossy().as_ref()));
        let link_key = lk.finish();
        let last = self.get(link_key).map(|d| d.to_vec());
        let obj_path = output.with_extension("o");
        let mut linked = false;
        let mut object_bytes = std::fs::metadata(&obj_path).map(|m| m.len() as usize).unwrap_or(obj.len());
        if last.as_deref() == Some(&obj_key.to_le_bytes()[..])
            && output.exists()
            && obj_path.exists()
        {
            tm.lap("link", "skipped (object unchanged)");
        } else {
            // 残す `.o` は機械語を zstd で圧縮したもの (次のビルドのキャッシュが機械語をここから引く)。
            // wild は圧縮したセクションをそのまま読む。ほかのリンカには圧縮しない `.o` を一時ファイルで渡す
            let objz = crate::elfz::compress_text(&obj);
            tm.lap("compress", format!("{} -> {} bytes", obj.len(), objz.len()));
            std::fs::write(&obj_path, &objz).map_err(|e| e.to_string())?;
            let wild = link::pick(self.opts.linker.as_deref())
                .is_ok_and(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("wild")));
            let used = if wild || objz.len() == obj.len() {
                link::link(self.opts.linker.as_deref(), &obj_path, output)?
            } else {
                let tmp = output.with_extension("tmp.o");
                std::fs::write(&tmp, &obj).map_err(|e| e.to_string())?;
                let r = link::link(self.opts.linker.as_deref(), &tmp, output);
                let _ = std::fs::remove_file(&tmp);
                r?
            };
            object_bytes = objz.len();
            linked = true;
            tm.lap("link", format!("{used}"));
        }
        self.put(link_key, obj_key.to_le_bytes().to_vec());
        let binary_bytes = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
        let _ = &a.sums;
        let _ = &a.sum_keys;
        Ok(BuildReport {
            fns_live: user_live.len() + insts.list.len(),
            fns_compiled: n_compiled,
            items_parsed: a.n_parsed,
            object_bytes,
            linked,
            binary_bytes,
            cache_bytes: 0,
        })
        .inspect(|_| {
            if self.opts.time {
                eprintln!(
                    "  {:<10} {:>8.2} ms  peak RSS {} KiB",
                    "total",
                    tm.start.elapsed().as_secs_f64() * 1e3,
                    peak_rss_kib()
                );
            }
        })
    }

    /// 今回のビルドで使ったエントリだけをファイルに書き戻し (自動 GC)、メモリからも捨てる。
    /// 書いたバイト数を返す。
    pub fn save_cache(&mut self, path: &Path) -> std::io::Result<usize> {
        if self.opts.no_cache {
            self.gc();
            return Ok(0);
        }
        let mut keys: Vec<Key> = self.used.iter().copied().collect();
        keys.sort_unstable();
        let mut entries: Vec<(Key, Vec<u8>)> = Vec::with_capacity(keys.len());
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
        let n = self.disk.save(path, &refs, 0)?;
        self.gc();
        Ok(n)
    }
}

/// ユーザのソースの後ろに、使われうる prelude のアイテム (Option / Result など) を付ける。
/// 後ろに付けるのでユーザのコードの行番号は変わらない。ユーザのコードの字句解析はここで 1 回だけ行い、
/// prelude の分だけ追加で字句解析して結合する
pub fn with_prelude<'a>(
    src: &'a str,
    cache: &mut crate::prelude::SelectCache,
) -> Result<(std::borrow::Cow<'a, str>, Vec<SrcItem>), String> {
    let orig = src;
    // `macro_rules!` を展開し (展開は 1 行に収めて後ろの行番号を変えない)、`mod m { .. }` は平らにする
    let tl = lexer::expand_thread_local(src);
    let src: &str = tl.as_deref().unwrap_or(src);
    let expanded = crate::macros::expand(src)?.or(tl.clone());
    let src: &str = expanded.as_deref().unwrap_or(src);
    let flat = lexer::flatten_modules(src).or(expanded.clone());
    let src: &str = flat.as_deref().unwrap_or(src);
    // 関数の中のアイテムはファイルの末尾へ
    let hoisted = lexer::hoist_nested_items(src).or(flat.clone());
    let src: &str = hoisted.as_deref().unwrap_or(src);
    let mut items = lexer::split(src)?;
    cache.user_items = items.len();
    let extra = crate::prelude::select_with(src, &items, cache);
    // (デバッグ用: 付けた prelude をソースの後ろに続く行番号つきで stderr に出す)
    if std::env::var_os("CRUSTC_DUMP_PRELUDE").is_some() {
        let l0 = src.bytes().filter(|&c| c == b'\n').count() + 2;
        for (i, l) in extra.lines().enumerate() {
            eprintln!("{:5} {l}", l0 + i);
        }
    }
    if extra.is_empty() {
        // prelude を使わなければソースはそのまま (コピーしない)
        return Ok((
            match &hoisted {
                Some(f) => std::borrow::Cow::Owned(f.clone()),
                None => std::borrow::Cow::Borrowed(orig),
            },
            items,
        ));
    }
    let base = src.len() + 1;
    let lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
    let n0 = items.len() as u32;
    for mut it in lexer::split(&extra)? {
        it.start += base;
        it.end += base;
        it.line += lines;
        for r in &mut it.idents {
            r.0 += base as u32;
            r.1 += base as u32;
        }
        it.kind = match it.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember(h + n0),
            ItemKind::TraitMember(h) => ItemKind::TraitMember(h + n0),
            k => k,
        };
        items.push(it);
    }
    let mut full = String::with_capacity(base + extra.len());
    full.push_str(src);
    full.push('\n');
    full.push_str(&extra);
    Ok((std::borrow::Cow::Owned(full), items))
}

/// 入力を小さな塊ごとに取り合う単純な並列 map (結果は入力順)。
/// `min_per_job` は 1 スレッドに割り当てる最小の要素数 (小さな入力でスレッドを立てすぎない)
fn par_map<T: Send, F>(
    jobs: usize,
    input: &[usize],
    min_per_job: usize,
    f: F,
) -> Result<Vec<T>, String>
where
    F: Fn(usize) -> Result<T, String> + Sync,
{
    let jobs = jobs.min(input.len() / min_per_job.max(1)).max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    // 取り合う塊の大きさ: 負荷が偏らないよう、スレッドあたり 8 回くらいは取りに来る大きさにする
    let step = (input.len() / (jobs * 8)).clamp(1, 16);
    let chunks: Vec<Result<Vec<(usize, T)>, String>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..jobs)
            .map(|_| {
                s.spawn(|| {
                    let mut out = vec![];
                    loop {
                        let i = next.fetch_add(step, std::sync::atomic::Ordering::Relaxed);
                        if i >= input.len() {
                            break;
                        }
                        for j in i..(i + step).min(input.len()) {
                            out.push((j, f(input[j])?));
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut all: Vec<(usize, T)> = vec![];
    for c in chunks {
        all.extend(c?);
    }
    all.sort_by_key(|(j, _)| *j);
    Ok(all.into_iter().map(|(_, t)| t).collect())
}

#[allow(clippy::too_many_arguments)]
fn compile_one(
    w: &mut Worker,
    src: &str,
    items: &[SrcItem],
    prog: &Program,
    insts: &Mutex<Insts>,
    id: u32,
    pre: Option<syn::Item>,
    isa: &dyn TargetIsa,
) -> Result<Blob, String> {
    let body = insts.lock().unwrap().body_of(id);
    let it = &items[prog.item_of[body as usize].unwrap() as usize];
    let item = match pre {
        Some(i) => i,
        None => front::parse_item(src, it)?,
    };
    let syn::Item::Fn(f) = item else {
        unreachable!()
    };
    let f = front::fn_ast(prog, f, body);
    w.line_base = it.line as u32 - 1;
    w.compile(prog, insts, id, &f, isa)
        .map_err(|e| front::relocate_err(e, it))
    // f (AST) はここで捨てる: 同時に生きている AST は常に 1 関数分だけ
}

#[allow(clippy::too_many_arguments)]
fn compile_all(
    o: &Options,
    src: &str,
    items: &[SrcItem],
    prog: &Program,
    insts: &Mutex<Insts>,
    misses: &[u32],
    mut parsed: HashMap<usize, syn::Item>,
    isa: &OwnedTargetIsa,
) -> Result<Vec<(u32, Blob)>, String> {
    // ミスが少ない (= インクリメンタル) ときはスレッドを起動しない。
    // syn の AST は !Send なので、並列時は各スレッドが担当アイテムだけを parse する。
    let jobs = o.jobs.min(misses.len() / 32).max(1);
    if jobs == 1 {
        let mut w = Worker::new();
        return misses
            .iter()
            .map(|&id| {
                let body = insts.lock().unwrap().body_of(id);
                let pre = parsed.remove(&(prog.item_of[body as usize].unwrap() as usize));
                Ok((
                    id,
                    compile_one(&mut w, src, items, prog, insts, id, pre, &**isa)?,
                ))
            })
            .collect();
    }
    drop(parsed);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..jobs)
            .map(|_| {
                let isa: Arc<dyn TargetIsa> = isa.clone();
                let next = &next;
                s.spawn(move || -> Result<Vec<(u32, Blob)>, String> {
                    let mut w = Worker::new();
                    let mut out = vec![];
                    loop {
                        // 小さな塊ごとに取りに行く (work stealing 的に負荷を均す)
                        let i = next.fetch_add(8, std::sync::atomic::Ordering::Relaxed);
                        if i >= misses.len() {
                            break;
                        }
                        for &id in &misses[i..(i + 8).min(misses.len())] {
                            out.push((
                                id,
                                compile_one(&mut w, src, items, prog, insts, id, None, &*isa)?,
                            ));
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        let mut out = vec![];
        for h in hs {
            out.extend(h.join().map_err(|_| "worker panicked".to_string())??);
        }
        Ok(out)
    })
}

/// 型検査だけを (ミスが多ければ並列に) 行う。結果は ids と同じ順
#[allow(clippy::too_many_arguments)]
fn check_all(
    o: &Options,
    src: &str,
    items: &[SrcItem],
    prog: &Program,
    insts: &Mutex<Insts>,
    ids: &[u32],
    mut parsed: HashMap<usize, syn::Item>,
    isa: &OwnedTargetIsa,
) -> Vec<Result<Vec<u32>, String>> {
    let body = |id: u32| insts.lock().unwrap().body_of(id);
    let one = |w: &mut Worker, id: u32, pre: Option<syn::Item>| -> Result<Vec<u32>, String> {
        let it = &items[prog.item_of[body(id) as usize].unwrap() as usize];
        let item = match pre {
            Some(i) => i,
            None => front::parse_item(src, it)?,
        };
        let syn::Item::Fn(f) = item else {
            return Ok(vec![]);
        };
        let f = front::fn_ast(prog, f, body(id));
        w.check(prog, insts, id, &f, &**isa)
            .map_err(|e| front::relocate_err(e, it))
    };
    if ids.len() <= KEEP_PARSED {
        let mut w = Worker::new();
        return ids
            .iter()
            .map(|&id| {
                let pre = parsed.remove(&(prog.item_of[body(id) as usize].unwrap() as usize));
                one(&mut w, id, pre)
            })
            .collect();
    }
    drop(parsed);
    // 各スレッドが Worker を 1 つずつ持てるよう、塊ごとに処理する
    let chunk = 16;
    let starts: Vec<usize> = (0..ids.len()).step_by(chunk).collect();
    par_map(o.jobs, &starts, 2, |st| {
        let mut w = Worker::new();
        Ok(ids[st..(st + chunk).min(ids.len())]
            .iter()
            .map(|&id| one(&mut w, id, None))
            .collect::<Vec<_>>())
    })
    .map(|v: Vec<Vec<Result<Vec<u32>, String>>>| v.into_iter().flatten().collect())
    .unwrap_or_else(|e| vec![Err(e)])
}

fn emit_object(
    prog: &Program,
    insts: &Insts,
    live: &[bool],
    user_live: &[u32],
    blobs: &FxMap<u32, Blob>,
    isa: OwnedTargetIsa,
) -> Result<(Vec<u8>, FxSet<u32>), String> {
    let cc = isa.default_call_conv();
    let sym_align = isa.symbol_alignment();
    let mut fn_off: FxMap<u32, u64> = FxMap::default();
    let mut text_end = 0u64;
    let mut first_fn: Option<cranelift_module::FuncId> = None;
    let mut b = ObjectBuilder::new(isa, "exr", cranelift_module::default_libcall_names())
        .map_err(|e| e.to_string())?;
    // 関数は 1 つの .text に並べる (関数ごとのセクションは 1 関数あたり section header 128 byte。
    // panic の表がほとんどの関数を参照するので --gc-sections はもともと効かない)
    b.per_function_section(false);
    let mut m = ObjectModule::new(b);
    // 置く関数: main / export から、機械語のリロケーションと vtable を辿って届くものだけ
    // (リンカの --gc-sections の代わり。呼ばれないクロージャや helper のインスタンスを置かない)
    let mut reach: FxSet<u32> = FxSet::default();
    let mut stack: Vec<u32> = user_live
        .iter()
        .copied()
        .filter(|&u| {
            let s = &prog.sigs[u as usize];
            s.is_main || s.export
        })
        .collect();
    while let Some(u) = stack.pop() {
        if !reach.insert(u) {
            continue;
        }
        let Some(b) = blobs.get(&u) else { continue };
        for r in &b.relocs {
            if let Target::Func(f) = r.target {
                stack.push(f);
            }
        }
        for vt in &b.vtables {
            stack.extend(vt.iter().copied().filter(|&f| f != u32::MAX));
        }
    }
    let unreached = |u: u32| blobs.contains_key(&u) && !reach.contains(&u);
    let user_live: Vec<u32> = user_live.iter().copied().filter(|&u| !unreached(u)).collect();
    let user_live = &user_live[..];
    let mut ids: FxMap<u32, cranelift_module::FuncId> = FxMap::default();
    for (i, sig) in prog.sigs.iter().enumerate() {
        // ジェネリック関数そのものは ABI を持たない (インスタンスだけを置く)
        if !live[i] || sig.is_generic() || !sig.has_body || unreached(i as u32) {
            continue;
        }
        let linkage = if sig.is_extern {
            Linkage::Import
        } else if sig.is_main || sig.export {
            Linkage::Export
        } else {
            Linkage::Local
        };
        let id = m
            .declare_function(&sig.symbol, linkage, &codegen::clif_sig(sig, cc))
            .map_err(|e| e.to_string())?;
        ids.insert(i as u32, id);
    }
    // インスタンスはシンボル名順に並べる (登録順は並列実行で変わるが、.o は決定的にしたい)
    let mut order: Vec<u32> = (0..insts.list.len() as u32)
        .map(|i| insts.n_fns + i)
        .filter(|&i| !unreached(i))
        .collect();
    order.sort_by(|a, b| {
        insts
            .get(*a)
            .unwrap()
            .sig
            .symbol
            .cmp(&insts.get(*b).unwrap().sig.symbol)
    });
    // 同じ機械語になるインスタンスは 1 つだけ置く (`Vec<u32>::push` と `Vec<i32>::push` など。ICF)。
    // 比べるのは機械語・リロケーション (呼び先は代表に直して)・データ・vtable・panic の場所。呼び先が畳まれると
    // 呼び出し側も同じになりうるので、変わらなくなるまで (高々 4 回) 繰り返す
    let mut fold: FxMap<u32, u32> = FxMap::default();
    for _ in 0..4 {
        let canon = |f: u32, fold: &FxMap<u32, u32>| *fold.get(&f).unwrap_or(&f);
        let mut seen: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut changed = false;
        for &iid in &order {
            if fold.contains_key(&iid) {
                continue;
            }
            let b = &blobs[&iid];
            let sig = &insts.get(iid).unwrap().sig;
            let mut key = Vec::with_capacity(b.code.len() + 64);
            key.extend_from_slice(&b.align.to_le_bytes());
            key.extend_from_slice(&(b.code.len() as u32).to_le_bytes());
            key.extend_from_slice(&b.code);
            // (呼び出し規約も同じでなければならない: 引数・戻り値の ABI)
            key.extend_from_slice(format!("{:?}", codegen::clif_sig(sig, cc)).as_bytes());
            for r in &b.relocs {
                key.extend_from_slice(&r.offset.to_le_bytes());
                key.push(r.kind);
                key.extend_from_slice(&r.addend.to_le_bytes());
                match r.target {
                    Target::Func(f) => {
                        key.push(0);
                        key.extend_from_slice(&canon(f, &fold).to_le_bytes());
                    }
                    Target::SelfOffset(o) => {
                        key.push(1);
                        key.extend_from_slice(&o.to_le_bytes());
                    }
                    Target::Data(k) => {
                        key.push(2);
                        key.extend_from_slice(&b.datas[k as usize]);
                        key.push(0xFF);
                    }
                    Target::Static(c) => {
                        key.push(3);
                        key.extend_from_slice(&c.to_le_bytes());
                    }
                    Target::Vtable(k) => {
                        key.push(4);
                        for &f in &b.vtables[k as usize] {
                            key.extend_from_slice(&canon(f, &fold).to_le_bytes());
                        }
                        key.push(0xFF);
                    }
                }
            }
            for &(o, i, mi) in &b.traps {
                key.extend_from_slice(&o.to_le_bytes());
                key.extend_from_slice(&i.to_le_bytes());
                if mi != u32::MAX {
                    key.extend_from_slice(&b.datas[mi as usize]);
                }
                key.push(0xFE);
            }
            match seen.get(&key) {
                Some(&r) => {
                    fold.insert(iid, r);
                    changed = true;
                }
                None => {
                    seen.insert(key, iid);
                }
            }
        }
        if !changed {
            break;
        }
    }
    let folded: Vec<u32> = order.iter().copied().filter(|i| fold.contains_key(i)).collect();
    order.retain(|i| !fold.contains_key(i));
    for &iid in &order {
        let sig = &insts.get(iid).unwrap().sig;
        let id = m
            .declare_function(&sig.symbol, Linkage::Local, &codegen::clif_sig(sig, cc))
            .map_err(|e| e.to_string())?;
        ids.insert(iid, id);
    }
    // (畳んだインスタンスへの参照は代表へ: 代表が畳まれていても辿る)
    for &iid in &folded {
        let mut r = fold[&iid];
        while let Some(&n) = fold.get(&r) {
            r = n;
        }
        let id = ids[&r];
        ids.insert(iid, id);
    }
    // static: コードから参照されているもの (と、その初期値から参照されているもの) だけを置く
    let mut statics: HashMap<u32, cranelift_module::DataId> = HashMap::new();
    let mut todo: Vec<u32> = user_live
        .iter()
        .chain(&order)
        .flat_map(|u| {
            blobs[u].relocs.iter().filter_map(|r| {
                if let Target::Static(c) = r.target {
                    Some(c)
                } else {
                    None
                }
            })
        })
        .collect();
    todo.sort_unstable();
    todo.dedup();
    let mut defs = vec![];
    let mut trap_table: Option<cranelift_module::DataId> = None;
    while let Some(ci) = todo.pop() {
        if statics.contains_key(&ci) {
            continue;
        }
        let c = &prog.consts[ci as usize];
        let id = m
            .declare_data(
                &format!("_xS{}", c.name),
                Linkage::Local,
                // (static は mut でなくても書き込めるところに置く: OnceLock / Cell などの内部可変性)
                c.stat.is_some(),
                false,
            )
            .map_err(|e| e.to_string())?;
        statics.insert(ci, id);
        // panic の trap の表は、全関数を置いた後で中身を作る
        if c.name == "__EXR_TRAP_TABLE" {
            trap_table = Some(id);
            continue;
        }
        // (自前の構文木で評価する。失敗したら場所つきのメッセージを syn 版で作る)
        let dv = match crate::ast_consteval::eval_src(prog, &c.expr, c.ty) {
            Ok(dv) => dv,
            Err(e0) => {
                let e: syn::Expr = syn::parse_str(&c.expr).map_err(|e| e.to_string())?;
                let msg = match crate::consteval::eval(prog, &e, c.ty) {
                    Err(e) => e.to_string(),
                    Ok(_) => e0,
                };
                return Err(format!("in static `{}`: {msg}", c.name));
            }
        };
        fn statics_in(dv: &crate::ast_consteval::DataVal, out: &mut Vec<u32>) {
            for (_, r) in &dv.relocs {
                match r {
                    crate::ast_consteval::DRef::Static(c) => out.push(*c),
                    crate::ast_consteval::DRef::Anon(inner) => statics_in(inner, out),
                }
            }
        }
        statics_in(&dv, &mut todo);
        defs.push((id, dv));
    }
    // 無名データは内容で重複を除く (文字列リテラルの共有)
    let mut anon: HashMap<Vec<u8>, cranelift_module::DataId> = HashMap::new();
    fn define_val(
        m: &mut ObjectModule,
        id: cranelift_module::DataId,
        dv: crate::ast_consteval::DataVal,
        anon: &mut HashMap<Vec<u8>, cranelift_module::DataId>,
        statics: &HashMap<u32, cranelift_module::DataId>,
    ) -> Result<(), String> {
        let mut desc = cranelift_module::DataDescription::new();
        desc.define(dv.bytes.into_boxed_slice());
        desc.set_align(dv.align as u64);
        for (off, r) in dv.relocs {
            let target = match r {
                crate::ast_consteval::DRef::Static(c) => statics[&c],
                crate::ast_consteval::DRef::Anon(inner) if inner.relocs.is_empty() => {
                    anon_data(m, inner.bytes, inner.align, anon)?
                }
                crate::ast_consteval::DRef::Anon(inner) => {
                    let nid = m
                        .declare_anonymous_data(false, false)
                        .map_err(|e| e.to_string())?;
                    define_val(m, nid, inner, anon, statics)?;
                    nid
                }
            };
            let gv = desc.import_global_value(ModuleRelocTarget::from(target));
            desc.write_data_addr(off, gv, 0);
        }
        m.define_data(id, &desc).map_err(|e| e.to_string())
    }
    fn anon_data(
        m: &mut ObjectModule,
        bytes: Vec<u8>,
        align: u32,
        anon: &mut HashMap<Vec<u8>, cranelift_module::DataId>,
    ) -> Result<cranelift_module::DataId, String> {
        if let Some(&id) = anon.get(&bytes) {
            return Ok(id);
        }
        let id = m
            .declare_anonymous_data(false, false)
            .map_err(|e| e.to_string())?;
        let mut desc = cranelift_module::DataDescription::new();
        desc.define(bytes.clone().into_boxed_slice());
        desc.set_align(align.max(1) as u64);
        m.define_data(id, &desc).map_err(|e| e.to_string())?;
        anon.insert(bytes, id);
        Ok(id)
    }
    for (id, dv) in defs {
        define_val(&mut m, id, dv, &mut anon, &statics)?;
    }
    // vtable: 関数アドレスの列。内容で重複を除く。到達しない関数 (どこからも名前で
    // 呼ばれないメソッド) は 0 にしておく (dyn 経由の呼び出しもメソッド名で参照に入るので呼ばれない)
    let mut vts: HashMap<Vec<u32>, cranelift_module::DataId> = HashMap::new();
    for &uid in user_live.iter().chain(&order) {
        let blob = &blobs[&uid];
        let fid = ids[&uid];
        let mut data_ids = vec![];
        for d in &blob.datas {
            // 昇格した定数 (`&Rect { .. }`) も入るので、長さから決まる自然なアラインメントにする
            let al = 1u32 << d.len().trailing_zeros().min(3);
            data_ids.push(anon_data(&mut m, d.clone(), al, &mut anon)?);
        }
        let mut vt_ids = vec![];
        for vt in &blob.vtables {
            if let Some(&id) = vts.get(vt) {
                vt_ids.push(id);
                continue;
            }
            let id = m
                .declare_anonymous_data(false, false)
                .map_err(|e| e.to_string())?;
            let mut desc = cranelift_module::DataDescription::new();
            desc.define(vec![0u8; 8 * vt.len().max(1)].into_boxed_slice());
            desc.set_align(8);
            for (i, f) in vt.iter().enumerate() {
                if let Some(&fid) = ids.get(f) {
                    let fr = desc.import_function(ModuleRelocTarget::from(fid));
                    desc.write_function_addr(8 * i as u32, fr);
                }
            }
            m.define_data(id, &desc).map_err(|e| e.to_string())?;
            vts.insert(vt.clone(), id);
            vt_ids.push(id);
        }
        let relocs: Vec<ModuleReloc> = blob
            .relocs
            .iter()
            .map(|r| ModuleReloc {
                offset: r.offset,
                kind: RELOC_KINDS[r.kind as usize],
                addend: r.addend,
                name: match r.target {
                    Target::Func(t) => ModuleRelocTarget::from(*ids.get(&t).unwrap_or_else(|| panic!("internal: function `{}` is not declared", insts.get(t).map(|i| i.sig.symbol.clone()).unwrap_or_else(|| prog.names[t as usize].clone())))),
                    Target::SelfOffset(off) => ModuleRelocTarget::FunctionOffset(fid, off),
                    Target::Data(k) => ModuleRelocTarget::from(data_ids[k as usize]),
                    Target::Static(c) => ModuleRelocTarget::from(statics[&c]),
                    Target::Vtable(k) => ModuleRelocTarget::from(vt_ids[k as usize]),
                },
            })
            .collect();
        m.define_function_bytes(fid, blob.align as u64, &blob.code, &relocs)
            .map_err(|e| e.to_string())?;
        // (.text の中の位置: cranelift-object は定義した順に、関数のアラインメントに揃えて並べる)
        let al = (blob.align as u64).max(sym_align).max(1);
        text_end = text_end.div_ceil(al) * al;
        fn_off.insert(uid, text_end);
        text_end += blob.code.len() as u64;
        first_fn.get_or_insert(fid);
    }
    // panic の trap の表 (main が登録したハンドラが、発生したアドレスで種類・場所・メッセージを引く):
    // [関数の数, (関数のアドレス, 個数, (オフセット << 44 | (メッセージ番号 + 1) << 31 | 種類・行・列) * 個数) *,
    //  メッセージの数, (アドレス, 長さ) *]  (1 か所 8 byte。オフセットが 1 MiB を超える場所は載せない)
    if let Some(tid) = trap_table {
        let mut bytes: Vec<u8> = vec![0; 8];
        let uv = |out: &mut Vec<u8>, mut v: u64| {
            while v >= 0x80 {
                out.push(v as u8 | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        };
        let mut msgs: Vec<Vec<u8>> = vec![];
        let mut funcs: Vec<u8> = vec![];
        let mut nf = 0u64;
        let (mut prev_fn, mut prev_line) = (0u64, 0i64);
        for &uid in user_live.iter().chain(&order) {
            let blob = &blobs[&uid];
            if blob.traps.is_empty() {
                continue;
            }
            nf += 1;
            let fo = fn_off[&uid];
            uv(&mut funcs, fo - prev_fn);
            prev_fn = fo;
            uv(&mut funcs, blob.traps.len() as u64);
            let mut prev_site = 0u64;
            for &(off, info, mi) in &blob.traps {
                let gm = if mi == u32::MAX {
                    0
                } else {
                    let bytes = &blob.datas[mi as usize];
                    match msgs.iter().position(|x| x == bytes) {
                        Some(k) => k as u64 + 1,
                        None => {
                            msgs.push(bytes.clone());
                            msgs.len() as u64
                        }
                    }
                };
                let (kind, line, col) = ((info >> 27) as u64, ((info >> 11) & 0xFFFF) as i64, (info & 0x7FF) as u64);
                uv(&mut funcs, off as u64 - prev_site);
                prev_site = off as u64;
                uv(&mut funcs, kind | (gm << 4));
                let d = line - prev_line;
                prev_line = line;
                uv(&mut funcs, ((d << 1) ^ (d >> 63)) as u64);
                uv(&mut funcs, col);
            }
        }
        uv(&mut bytes, msgs.len() as u64);
        for mb in &msgs {
            uv(&mut bytes, mb.len() as u64);
            bytes.extend_from_slice(mb);
        }
        uv(&mut bytes, nf);
        bytes.extend_from_slice(&funcs);
        let mut desc = cranelift_module::DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        desc.set_align(8);
        // (表が参照するのは .text の先頭 (最初に置いた関数) だけ。関数の位置は先頭からのオフセット)
        if let Some(fid) = first_fn {
            let fr = desc.import_function(ModuleRelocTarget::from(fid));
            desc.write_function_addr(0, fr);
        }
        m.define_data(tid, &desc).map_err(|e| e.to_string())?;
    }
    let obj = m.finish().emit().map_err(|e| e.to_string())?;
    // (panic の表は関数の位置を自分で数えている: 実際の .o の位置と合っているか確かめる)
    if trap_table.is_some() {
        let syms: FxMap<String, u64> = crate::elfz::func_symbols(&obj)
            .unwrap_or_default()
            .into_iter()
            .map(|(n, _, v)| (n, v))
            .collect();
        for (&uid, &off) in &fn_off {
            let sym = match insts.get(uid) {
                Some(i) => &i.sig.symbol,
                None => &prog.sigs[uid as usize].symbol,
            };
            if syms.get(sym).is_some_and(|&v| v != off) {
                return Err(format!("internal: function `{sym}` is at {:#x} in .text, expected {off:#x}", syms[sym]));
            }
        }
    }
    Ok((obj, reach))
}

pub fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}
