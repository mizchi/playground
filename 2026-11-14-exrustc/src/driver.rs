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
            jobs: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
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
                return Diagnostic { line, col, msg: m.trim().to_string() };
            }
        }
        if let Some((l, m)) = s.split_once(':') {
            if let Ok(line) = l.trim().parse() {
                return Diagnostic { line, col: 1, msg: m.trim().to_string() };
            }
        }
        Diagnostic { line: 1, col: 1, msg: s.to_string() }
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
    mem: HashMap<Key, Vec<u8>>,
    used: HashSet<Key>,
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
            eprintln!("  {:<10} {:>8.2} ms  {}", what, (now - self.last).as_secs_f64() * 1e3, extra);
            self.last = now;
        }
    }
}

fn make_isa(o: &Options) -> OwnedTargetIsa {
    let mut f = settings::builder();
    f.set("opt_level", if o.opt { "speed_and_size" } else { "none" }).unwrap();
    f.set("regalloc_algorithm", if o.fast_regalloc { "single_pass" } else { "backtracking" })
        .unwrap();
    f.set("is_pic", "true").unwrap();
    // 開発ビルドではアンウィンド情報不要 (panic/unwind をまだ持たない)
    f.set("unwind_info", "false").unwrap();
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
    s.push(".exrc");
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
}

impl Session {
    pub fn new(opts: Options) -> Session {
        let isa = make_isa(&opts);
        Session { opts, isa, cid: compiler_id(), disk: Cache::empty(), mem: HashMap::new(), used: HashSet::new() }
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
        let mut sums: Vec<Option<Summary>> =
            sum_keys.iter().map(|&k| self.get(k).and_then(Summary::decode)).collect();
        let todo: Vec<usize> = (0..items.len()).filter(|&i| sums[i].is_none()).collect();
        let n_parsed = todo.len();
        let mut errors = vec![];
        let mut failed = HashSet::new();
        let mut record = |ii: usize, r: Result<Summary, String>, sums: &mut Vec<Option<Summary>>| match r {
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
                        self_ty: hs.impl_self_ty.clone().unwrap_or(front::TySyn::Named(n.clone())),
                        base: n,
                        impl_trait: hs.impl_trait.clone(),
                        generics: hs.impl_generics.clone(),
                        is_trait: false,
                    })
                }
                ItemKind::TraitMember(h) => sums[h as usize].as_ref().unwrap().trait_decl.clone().map(|n| front::Owner {
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
        let prog = Program::build(&items, &sums)?;
        let (refs, srefs): (Vec<Vec<u32>>, Vec<Vec<u32>>) = prog
            .item_of
            .iter()
            .map(|ii| ii.map(|ii| prog.refs(src, &items[ii as usize])).unwrap_or_default())
            .unzip();
        Ok(Analysis { items, sums, sum_keys, prog, refs, srefs, parsed, n_parsed, errors })
    }

    /// 関数 id の内容キー: 本体ハッシュ + 自分と参照先のシグネチャ + 名前で現れる struct
    fn fn_key(a: &Analysis, id: usize, base: Key) -> Key {
        let ii = a.prog.item_of[id].unwrap() as usize;
        let mut h = KeyHasher::new();
        h.add(&(base, a.items[ii].hash));
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
        let user_lines = src.lines().count() + 1;
        let out = match with_prelude(src) {
            Ok((full, items)) => self.check_inner(&full, items),
            Err(e) => vec![e],
        };
        // prelude の中の診断は出さない (ユーザのコードではない)
        let out: Vec<String> = out.into_iter().filter(|e| Diagnostic::parse(e).line <= user_lines).collect();
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
        let mut ids: Vec<u32> = (0..a.prog.names.len()).filter(|&i| Self::concrete(&a.prog, i)).map(|i| i as u32).collect();
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
            let results = check_all(&self.opts, src, &a.items, &a.prog, &insts, &tids, std::mem::take(&mut parsed), &self.isa);
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
        let r = with_prelude(src).and_then(|(full, items)| self.build_inner(&full, items, output));
        if r.is_err() {
            self.used.clear();
        }
        r
    }

    fn build_inner(&mut self, src: &str, items: Vec<SrcItem>, output: &Path) -> Result<BuildReport, String> {
        let t0 = Instant::now();
        let mut tm = Timer { on: self.opts.time, start: t0, last: t0 };
        let mut a = self.analyze(src, items)?;
        if let Some(e) = a.errors.first() {
            return Err(e.clone());
        }
        tm.lap("front", format!("{} items, {} parsed", a.items.len(), a.n_parsed));

        let live = a.prog.reachable(&a.refs);
        let mut base = KeyHasher::new();
        base.add(&(self.cid, 1u8));
        base.add(self.isa.triple().to_string().as_str());
        base.add(&(self.opts.opt, self.opts.fast_regalloc));
        let base = base.finish();
        let user_live: Vec<u32> =
            (0..a.prog.names.len()).filter(|&i| live[i] && Self::concrete(&a.prog, i)).map(|i| i as u32).collect();
        let n_user = a.prog.item_of.iter().filter(|i| i.is_some()).count();
        tm.lap("plan", format!("{} live fns, {} unreachable skipped", user_live.len(), n_user - user_live.len()));

        // 関数 → (キャッシュ or コンパイル)。その途中で要求されたインスタンスを、不動点まで同じように処理する
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut blobs: HashMap<u32, Blob> = HashMap::new();
        let mut parsed = std::mem::take(&mut a.parsed);
        let mut n_hit = 0;
        let mut n_compiled = 0;
        let mut wave = user_live.clone();
        while !wave.is_empty() {
            let mut misses = vec![];
            let mut keys = HashMap::new();
            for &id in &wave {
                let k = Self::key_of(&a, &insts, id, base);
                keys.insert(id, k);
                match self.get(k).and_then(|d| cache::decode_blob(d, &a.prog, &insts)) {
                    Some(b) => {
                        n_hit += 1;
                        blobs.insert(id, b);
                    }
                    None => misses.push(id),
                }
            }
            let compiled =
                compile_all(&self.opts, src, &a.items, &a.prog, &insts, &misses, std::mem::take(&mut parsed), &self.isa)?;
            n_compiled += misses.len();
            for (id, b) in compiled {
                let mut buf = vec![];
                cache::encode_blob(&b, &a.prog, &insts.lock().unwrap(), &mut buf);
                self.put(keys[&id], buf);
                blobs.insert(id, b);
            }
            wave = std::mem::take(&mut insts.lock().unwrap().pending);
            wave.retain(|id| !blobs.contains_key(id));
        }
        let insts = insts.into_inner().unwrap();
        tm.lap(
            "codegen",
            format!("{n_hit} hit / {n_compiled} compiled ({} generic instances)", insts.list.len()),
        );

        let obj = emit_object(&a.prog, &insts, &live, &user_live, &blobs, self.isa.clone())?;
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
        if last.as_deref() == Some(&obj_key.to_le_bytes()[..]) && output.exists() && obj_path.exists() {
            tm.lap("link", "skipped (object unchanged)");
        } else {
            std::fs::write(&obj_path, &obj).map_err(|e| e.to_string())?;
            let used = link::link(self.opts.linker.as_deref(), &obj_path, output)?;
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
            object_bytes: obj.len(),
            linked,
            binary_bytes,
            cache_bytes: 0,
        })
        .inspect(|_| {
            if self.opts.time {
                eprintln!("  {:<10} {:>8.2} ms  peak RSS {} KiB", "total", tm.start.elapsed().as_secs_f64() * 1e3, peak_rss_kib());
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
fn with_prelude(src: &str) -> Result<(String, Vec<SrcItem>), String> {
    let mut items = lexer::split(src)?;
    let extra = crate::prelude::select(src, &items);
    if extra.is_empty() {
        return Ok((src.to_string(), items));
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
    Ok((full, items))
}

/// 入力を小さな塊ごとに取り合う単純な並列 map (結果は入力順)。
/// `min_per_job` は 1 スレッドに割り当てる最小の要素数 (小さな入力でスレッドを立てすぎない)
fn par_map<T: Send, F>(jobs: usize, input: &[usize], min_per_job: usize, f: F) -> Result<Vec<T>, String>
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
    let syn::Item::Fn(f) = item else { unreachable!() };
    w.compile(prog, insts, id, &f, isa).map_err(|e| front::relocate_err(e, it))
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
                Ok((id, compile_one(&mut w, src, items, prog, insts, id, pre, &**isa)?))
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
                            out.push((id, compile_one(&mut w, src, items, prog, insts, id, None, &*isa)?));
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
        let syn::Item::Fn(f) = item else { return Ok(vec![]) };
        w.check(prog, insts, id, &f, &**isa).map_err(|e| front::relocate_err(e, it))
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
        Ok(ids[st..(st + chunk).min(ids.len())].iter().map(|&id| one(&mut w, id, None)).collect::<Vec<_>>())
    })
    .map(|v: Vec<Vec<Result<Vec<u32>, String>>>| v.into_iter().flatten().collect())
    .unwrap_or_else(|e| vec![Err(e)])
}

fn emit_object(
    prog: &Program,
    insts: &Insts,
    live: &[bool],
    user_live: &[u32],
    blobs: &HashMap<u32, Blob>,
    isa: OwnedTargetIsa,
) -> Result<Vec<u8>, String> {
    let cc = isa.default_call_conv();
    let mut b = ObjectBuilder::new(isa, "exr", cranelift_module::default_libcall_names())
        .map_err(|e| e.to_string())?;
    // 関数ごとにセクションを分ける → リンカの --gc-sections が効く
    b.per_function_section(true);
    let mut m = ObjectModule::new(b);
    let mut ids = HashMap::new();
    for (i, sig) in prog.sigs.iter().enumerate() {
        // ジェネリック関数そのものは ABI を持たない (インスタンスだけを置く)
        if !live[i] || sig.is_generic() || !sig.has_body {
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
    let mut order: Vec<u32> = (0..insts.list.len() as u32).map(|i| insts.n_fns + i).collect();
    order.sort_by(|a, b| insts.get(*a).unwrap().sig.symbol.cmp(&insts.get(*b).unwrap().sig.symbol));
    for &iid in &order {
        let sig = &insts.get(iid).unwrap().sig;
        let id = m
            .declare_function(&sig.symbol, Linkage::Local, &codegen::clif_sig(sig, cc))
            .map_err(|e| e.to_string())?;
        ids.insert(iid, id);
    }
    for &uid in user_live.iter().chain(&order) {
        let blob = &blobs[&uid];
        let fid = ids[&uid];
        let relocs: Vec<ModuleReloc> = blob
            .relocs
            .iter()
            .map(|r| ModuleReloc {
                offset: r.offset,
                kind: RELOC_KINDS[r.kind as usize],
                addend: r.addend,
                name: match r.target {
                    Target::Func(t) => ModuleRelocTarget::from(ids[&t]),
                    Target::SelfOffset(off) => ModuleRelocTarget::FunctionOffset(fid, off),
                },
            })
            .collect();
        m.define_function_bytes(fid, blob.align as u64, &blob.code, &relocs)
            .map_err(|e| e.to_string())?;
    }
    m.finish().emit().map_err(|e| e.to_string())
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
