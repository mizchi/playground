//! syn と Cranelift を使わないドライバ: ソース → (lexer / macros / prelude) → ast_summary → Program →
//! acg (自前の構文木のコード生成) → minclif の ELF の .o → リンク。
//! driver.rs と同じ手順を、外部クレートに依存しないモジュールだけで行う (crustc 自身でもコンパイルできる)。
//! キャッシュと並列化はまだない。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::acg::{self, Blob, ItemFn, Target};
use crate::key::{Key, KeyHasher};
use crate::fxhash::{FxMap, FxSet};
use crate::lexer::{self, ItemKind, SrcItem};
use crate::minclif::{DataDef, Reloc};
use crate::parse::{Block, Expr, Pat, Stmt};
use crate::program::{Insts, Program, TyEnv};
use crate::types::{Summary, Ty};

pub type R<T> = Result<T, String>;

/// ユーザのソースの後ろに、使われうる prelude のアイテムを付ける (driver::with_prelude と同じ)。
/// 返り値は (全体のソース, アイテム, ユーザのアイテムの数)
pub fn with_prelude(src: &str) -> R<(String, Vec<SrcItem>, usize)> {
    with_prelude_in(src, &[], &mut vec![], None, &mut Default::default()).map(|(s, i, u, _)| (s, i, u))
}

/// crates: ソースの中のクレートのモジュールの名前 (lexer::rename_module_collisions_in)
/// fix: モジュールの名前を変えて桁がずれた所 (lexer::rename_module_collisions_fix)
/// cfg: マクロを展開したものの中の `#[cfg(..)]` を評価する設定
/// 返り値の 4 つ目は prelude の先頭の行 (prelude の中の行 = 結合したソースの行 - これ)
/// 前処理の、編集をまたいで覚えておくもの (LSP の didChange で変わっていない所を作り直さない)
#[derive(Default)]
pub struct FrontMemo {
    pub sel: crate::prelude::SelectCache,
    /// 前回の split の入力と結果
    pub split: Option<(String, Vec<SrcItem>)>,
}

fn clone_item(it: &SrcItem) -> SrcItem {
    SrcItem { start: it.start, end: it.end, line: it.line, hash: it.hash, idents: it.idents.clone(), kind: it.kind }
}

fn count_nl(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c == b'\n').count()
}

/// lexer::split を、前回の入力と結果から差分で作る: 前後の変わっていない部分のアイテムはそのまま (後ろは位置をずらす) 使い、
/// 間だけを split し直す。区切りは、空白の後から始まるトップレベルのアイテム (impl / trait のブロックは丸ごと) の先頭。
/// 間の終わりがコメント・文字列の途中になりうるときや、split が失敗するときは全体を split する
#[doc(hidden)]
pub fn split_inc(src: &str, memo: &mut Option<(String, Vec<SrcItem>)>) -> R<Vec<SrcItem>> {
    let r = match memo.as_ref() {
        Some((old, items)) if std::env::var_os("CRUSTC_NO_SPLIT_INC").is_none() => split_diff(old, items, src),
        _ => None,
    };
    let items = match r {
        Some(v) => v,
        None => lexer::split(src)?,
    };
    if std::env::var_os("CRUSTC_SPLIT_VERIFY").is_some() {
        let full = lexer::split(src)?;
        let same = full.len() == items.len()
            && full.iter().zip(&items).all(|(a, b)| a.start == b.start && a.end == b.end && a.line == b.line && a.hash == b.hash && a.idents == b.idents && a.kind == b.kind);
        if !same {
            return Err("internal: incremental split differs from the full split".into());
        }
    }
    *memo = Some((src.to_string(), items.iter().map(clone_item).collect()));
    Ok(items)
}

fn split_diff(old: &str, items: &[SrcItem], new: &str) -> Option<Vec<SrcItem>> {
    let (p, n) = (old.as_bytes(), new.as_bytes());
    if p == n {
        return Some(items.iter().map(clone_item).collect());
    }
    let lim = p.len().min(n.len());
    let mut pre = 0;
    while pre < lim && p[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < lim - pre && p[p.len() - 1 - suf] == n[n.len() - 1 - suf] {
        suf += 1;
    }
    // 区切り: (アイテムの番号, 位置)
    let mut groups: Vec<(usize, usize)> = vec![];
    for (k, it) in items.iter().enumerate() {
        let top = !matches!(it.kind, ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
        if top && (it.start == 0 || p[it.start - 1].is_ascii_whitespace()) {
            groups.push((k, it.start));
        }
    }
    // 前: 区切りの範囲 (次の区切りまで) が変わった所より前にあるもの
    let mut j = 0;
    while j + 1 < groups.len() && groups[j + 1].1 <= pre {
        j += 1;
    }
    // (groups[j] が変わった所を含む。先頭のアイテムより前が変わったら最初から)
    let (keep_n, a) = if groups.is_empty() || groups[j].1 > pre { (0, 0) } else { (groups[j].0, groups[j].1) };
    // 後ろ: 変わった所より後から始まる最初の区切り
    let chg_end = p.len() - suf;
    let m = groups.iter().position(|g| g.1 > chg_end && g.1 > a);
    let (tail_k, b_old) = match m {
        Some(m) => (groups[m].0, groups[m].1),
        None => (items.len(), p.len()),
    };
    let b_new = (b_old + n.len()).checked_sub(p.len())?;
    if b_new < a {
        return None;
    }
    let mid = &new[a..b_new];
    let mut mids = lexer::split(mid).ok()?;
    // (間の終わりの、アイテムの後ろ: コメントや文字列が開いたままかもしれないものがあれば使わない)
    let last_end = mids.last().map(|x| x.end).unwrap_or(0);
    let gap = &mid.as_bytes()[last_end..];
    if gap.windows(2).any(|w| w == b"/*") || gap.iter().any(|&c| c == b'"' || c == b'\'' || c == b'#') {
        return None;
    }
    let mut out: Vec<SrcItem> = Vec::with_capacity(keep_n + mids.len() + items.len() - tail_k);
    out.extend(items[..keep_n].iter().map(clone_item));
    let lines_a = count_nl(&n[..a]);
    for it in &mut mids {
        it.start += a;
        it.end += a;
        it.line += lines_a;
        for x in &mut it.idents {
            x.0 += a as u32;
            x.1 += a as u32;
        }
        it.kind = match it.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember(h + keep_n as u32),
            ItemKind::TraitMember(h) => ItemKind::TraitMember(h + keep_n as u32),
            k => k,
        };
    }
    out.extend(mids);
    let dk = out.len() as i64 - tail_k as i64;
    let dpos = n.len() as i64 - p.len() as i64;
    let dline = count_nl(&n[..b_new]) as i64 - count_nl(&p[..b_old]) as i64;
    for it in &items[tail_k..] {
        let mut x = clone_item(it);
        x.start = (x.start as i64 + dpos) as usize;
        x.end = (x.end as i64 + dpos) as usize;
        x.line = (x.line as i64 + dline) as usize;
        for y in &mut x.idents {
            y.0 = (y.0 as i64 + dpos) as u32;
            y.1 = (y.1 as i64 + dpos) as u32;
        }
        x.kind = match x.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember((h as i64 + dk) as u32),
            ItemKind::TraitMember(h) => ItemKind::TraitMember((h as i64 + dk) as u32),
            k => k,
        };
        out.push(x);
    }
    Some(out)
}

pub fn with_prelude_in(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>, cfg: Option<&crate::cfg::CfgSet>, cache: &mut FrontMemo) -> R<(String, Vec<SrcItem>, usize, usize)> {
    let mut tm = Timer { on: std::env::var_os("CRUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
    let tl = lexer::expand_thread_local(src);
    let src: &str = tl.as_deref().unwrap_or(src);
    let expanded = crate::macros::expand_cfg(src, cfg)?.or(tl.clone());
    let src: &str = expanded.as_deref().unwrap_or(src);
    tm.lap("macros", String::new());
    if let Some(p) = std::env::var_os("CRUSTC_DUMP_PRE") {
        let _ = std::fs::write(p, src);
    }
    let flat = lexer::flatten_modules_fix(src, crates, fix).or(expanded.clone());
    let src: &str = flat.as_deref().unwrap_or(src);
    tm.lap("flatten", String::new());
    if tm.on {
        let n = lexer::tokenize(src.as_bytes()).len();
        tm.lap("(tokenize)", format!("{n} tokens"));
    }
    let hoisted = lexer::hoist_nested_items(src).or(flat.clone());
    let src: &str = hoisted.as_deref().unwrap_or(src);
    tm.lap("hoist", String::new());
    let cv = crate::ctors::rewrite_ctor_values(src);
    let src: &str = cv.as_deref().unwrap_or(src);
    tm.lap("ctor", String::new());
    let mut items = split_inc(src, &mut cache.split)?;
    let user = items.len();
    tm.lap("split", String::new());
    let extra = crate::prelude::select_with(src, &items, &mut cache.sel);
    tm.lap("prelude", String::new());
    if extra.is_empty() {
        return Ok((src.to_string(), items, user, 0));
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
    Ok((full, items, user, lines))
}

/// 解析の結果
pub struct Analysis {
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
    pub backend: Option<acg::ClBackend>,
    /// `#[cfg(feature = "..")]` で真になる feature (1 つのクレートのビルド。ワークスペースはクレートごとに CrateSpec が持つ)
    pub features: Vec<String>,
}

impl Default for Options2 {
    fn default() -> Self {
        Options2 { opt: false, jobs: cpu_count(), linker: None, no_cache: false, time: false, backend: None, features: vec![] }
    }
}

impl Options2 {
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
    #[link_name = "clock_gettime"]
    fn c_clock_gettime(clk: i32, ts: *mut i64) -> i32;
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
fn now_ns() -> u64 {
    let mut ts = [0i64; 2];
    // SAFETY: CLOCK_MONOTONIC (1)。ts は timespec と同じ並び
    unsafe { c_clock_gettime(1, ts.as_mut_ptr()) };
    ts[0] as u64 * 1_000_000_000 + ts[1] as u64
}

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
struct Timer {
    on: bool,
    last: u64,
    start: u64,
}

impl Timer {
    fn lap(&mut self, what: &str, extra: String) {
        if !self.on {
            return;
        }
        let t = now_ns();
        eprintln!("  {:<10} {:>8.2} ms  {}", what, (t - self.last) as f64 / 1e6, extra);
        self.last = t;
    }
}

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
    /// 複数のクレートのうちの 1 つをコンパイルするとき
    pub ctx: CrateCtx,
}

impl Session2 {
    pub fn new(opts: Options2) -> Session2 {
        Session2 { opts, cid: compiler_id(), disk: crate::cache2::Cache2::empty(), mem: FxMap::default(), used: FxSet::default(), fresh: vec![], ctx: CrateCtx::default(), sel: Default::default() }
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
        let mut a = Analysis { src, items, prog, refs, srefs, crefs, n_parsed, ext_items, inl: acg::Inline::default(), prelude_line0, dep_ranges: self.ctx.dep_ranges.clone(), asts: Mutex::new(FxMap::default()), encl: Mutex::new(FxMap::default()), sig_keys: Mutex::new(Vec::new()) };
        let cl = self.opts.backend;
        if self.opts.opt && std::env::var_os("CRUSTC_NO_INLINE").is_none() {
            a.inl = inline_bodies(&a);
        }
        a.inl.cl = cl;
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

/// ソースから ELF の .o を作る (キャッシュも並列化もしない)
pub fn build_object(src0: &str, opt: bool) -> R<Vec<u8>> {
    let mut s = Session2::new(Options2 { opt, no_cache: true, jobs: 1, ..Default::default() });
    let a = s.analyze(src0)?;
    s.compile_object(&a, &mut Report2::default(), None)
}

/// ソースから実行ファイルを作る (キャッシュも並列化もしない)
pub fn build(src: &str, output: &std::path::Path, opt: bool, linker: Option<&str>) -> R<()> {
    let mut s = Session2::new(Options2 { opt, no_cache: true, jobs: 1, linker: linker.map(|l| l.to_string()), ..Default::default() });
    s.build(src, output).map(|_| ())
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
            h.add(&(self.opts.backend.is_some() as u8));
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
            h.add(&(self.opts.backend.is_some() as u8));
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
        h.add(&(self.opts.backend.is_some() as u8));
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

    /// ids をコンパイルする (jobs > 1 で数が多ければ fork した子プロセスで分けて)。結果は ids と同じ順
    fn compile_many(&self, a: &Analysis, insts: &Mutex<Insts>, ids: &[u32]) -> R<Vec<Blob>> {
        let jobs = self.opts.jobs.min(ids.len() / 16).max(1);
        if jobs <= 1 {
            let mut out = vec![];
            for &id in ids {
                out.push(compile_id(a, insts, id)?);
            }
            return Ok(out);
        }
        // 子プロセス k は ids[i] (i % jobs == k) をコンパイルし、(番号, 失敗か, 中身) の並びを pipe に書く
        let mut kids = vec![];
        for k in 0..jobs {
            let mut fds = [0i32; 2];
            // SAFETY: fds は 2 つの int
            if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                return Err("pipe failed".into());
            }
            // SAFETY: 子はコンパイルして書くだけで _exit する
            let pid = unsafe { c_fork() };
            if pid < 0 {
                return Err("fork failed".into());
            }
            if pid == 0 {
                unsafe { c_close(fds[0]) };
                let mut buf = vec![];
                let mut i = k;
                while i < ids.len() {
                    crate::cache2::put_uv(&mut buf, i as u64);
                    match compile_id(a, insts, ids[i]) {
                        Ok(b) => {
                            let mut e = vec![];
                            {
                                let g = insts.lock().unwrap();
                                crate::cache2::encode_blob(&b, &a.prog, &*g, &mut e);
                            }
                            buf.push(0);
                            crate::cache2::put_uv(&mut buf, e.len() as u64);
                            buf.extend_from_slice(&e);
                        }
                        Err(m) => {
                            buf.push(1);
                            crate::cache2::put_uv(&mut buf, m.len() as u64);
                            buf.extend_from_slice(m.as_bytes());
                        }
                    }
                    i += jobs;
                }
                write_all(fds[1], &buf);
                unsafe { c_close(fds[1]) };
                unsafe { c_exit(0) };
            }
            unsafe { c_close(fds[1]) };
            kids.push((pid, fds[0]));
        }
        let mut res: Vec<Option<Result<Vec<u8>, String>>> = (0..ids.len()).map(|_| None).collect();
        for &(pid, fd) in &kids {
            let d = read_all(fd);
            unsafe { c_close(fd) };
            let mut st = 0i32;
            unsafe { c_waitpid(pid, &mut st, 0) };
            let mut r = crate::cache2::Rd { d: &d, pos: 0 };
            while r.pos < d.len() {
                let (Some(i), Some(fail), Some(n)) = (r.uv(), r.u8(), r.uv()) else { break };
                let Some(body) = r.take(n as usize) else { break };
                let v = if fail == 0 { Ok(body.to_vec()) } else { Err(String::from_utf8_lossy(body).into_owned()) };
                if let Some(slot) = res.get_mut(i as usize) {
                    *slot = Some(v);
                }
            }
        }
        // (順に読み戻す: 子が要求したインスタンスはここで要求し直す。失敗は ids の順で最初のもの)
        let mut out = vec![];
        for (i, r) in res.into_iter().enumerate() {
            match r {
                Some(Ok(d)) => out.push(crate::cache2::decode_blob(&d, &a.prog, insts).ok_or("internal: cannot decode a compiled function")?),
                Some(Err(m)) => return Err(m),
                None => {
                    // (子が落ちた: その関数はここでコンパイルする (エラーが出るならそれが出る))
                    out.push(compile_id(a, insts, ids[i])?);
                }
            }
        }
        Ok(out)
    }

    /// 解析済みのプログラムの .o を作る (キャッシュを引き、なければコンパイルしてキャッシュに入れる)
    fn compile_object(&mut self, a: &Analysis, rep: &mut Report2, tm: Option<&mut Timer>) -> R<Vec<u8>> {
        let mut live = a.prog.reachable(&a.refs);
        let mut user_live: Vec<u32> = (0..a.prog.names.len()).filter(|&i| live[i] && concrete(&a.prog, i) && !a.is_ext(i as u32)).map(|i| i as u32).collect();
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut blobs: FxMap<u32, Blob> = FxMap::default();
        let mut wave = user_live.clone();
        while !wave.is_empty() {
            let cur = wave.clone();
            let mut misses = vec![];
            let mut keys: FxMap<u32, Key> = FxMap::default();
            for &id in &wave {
                if blobs.contains_key(&id) || keys.contains_key(&id) {
                    continue;
                }
                let k = self.key_of(a, &insts, id);
                keys.insert(id, k);
                // (場所を書き込んだ関数は、アイテムの開始行が変わっていたら使わない)
                let lb = {
                    let g = insts.lock().unwrap();
                    a.line_base_for(&*g, id)
                };
                let raw = self.get(k);
                if std::env::var_os("CRUSTC_LOG_COMPILED").is_some() && raw.is_some() {
                    let d = crate::cache2::decode_blob(raw.as_ref().unwrap(), &a.prog, &insts);
                    match &d {
                        None => eprintln!("decode-fail {}", a.prog.names[insts.lock().unwrap().body_of(id) as usize]),
                        Some(b) if !(b.line_base.is_none() || b.line_base == lb) => eprintln!("line-mismatch {}", a.prog.names[insts.lock().unwrap().body_of(id) as usize]),
                        _ => {}
                    }
                }
                let hit = raw.and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)).filter(|b| b.line_base.is_none() || b.line_base == lb);
                match hit {
                    Some(b) => {
                        rep.fns_hit += 1;
                        blobs.insert(id, b);
                    }
                    None => misses.push(id),
                }
            }
            if std::env::var_os("CRUSTC_LOG_COMPILED").is_some() {
                let g = insts.lock().unwrap();
                for &id in &misses {
                    let b = g.body_of(id);
                    let args = g.get(id).map(|i| i.args.iter().map(|&t| a.prog.tn(t)).collect::<Vec<_>>().join(",")).unwrap_or_default();
                    eprintln!("compiled {}<{args}>", a.prog.names[b as usize]);
                }
            }
            let compiled = self.compile_many(a, &insts, &misses)?;
            rep.fns_compiled += misses.len();
            for (&id, b) in misses.iter().zip(compiled) {
                if !self.opts.no_cache {
                    let mut e = vec![];
                    {
                                let g = insts.lock().unwrap();
                                crate::cache2::encode_blob(&b, &a.prog, &*g, &mut e);
                            }
                    self.put(keys[&id], e);
                }
                blobs.insert(id, b);
            }
            wave = std::mem::take(&mut insts.lock().unwrap().pending);
            // (展開した本体が呼ぶ関数は、呼んだ側の名前からは届かない: 再配置が指す総称でない関数もコンパイルする)
            // (vtable の欄も: `Box<dyn Shape>` にした自分の型のメソッドは、名前で届くとは限らない
            //  (呼ぶのが依存するクレートの関数の中で、その本体はインターフェースでは消してある))
            for &id in &cur {
                if let Some(b) = blobs.get(&id) {
                    let fs = b.relocs.iter().filter_map(|r| if let Target::Func(f) = r.target { Some(f) } else { None });
                    let vs = b.vtables.iter().flat_map(|v| v.iter().copied());
                    for f in fs.chain(vs) {
                        if f != u32::MAX && (f as usize) < a.prog.names.len() && concrete(&a.prog, f as usize) && !a.is_ext(f) && !blobs.contains_key(&f) && !live[f as usize] {
                            live[f as usize] = true;
                            user_live.push(f);
                            wave.push(f);
                        }
                    }
                }
            }
            wave.retain(|id| !blobs.contains_key(id));
            let starts: Vec<u32> = {
                let g = insts.lock().unwrap();
                wave.iter().map(|&id| g.body_of(id)).collect()
            };
            let mut stack: Vec<u32> = starts.iter().flat_map(|&b| a.refs[b as usize].iter().copied()).collect();
            if starts.iter().any(|&b| matches!(a.prog.names[b as usize].as_str(), "__exr_display" | "__exr_debug")) {
                stack.extend((0..a.prog.names.len() as u32).filter(|&i| matches!(a.prog.sigs[i as usize].impl_trait.as_deref(), Some("Display" | "Debug"))));
            }
            while let Some(i) = stack.pop() {
                if std::mem::replace(&mut live[i as usize], true) {
                    continue;
                }
                if a.is_ext(i) {
                    continue;
                }
                stack.extend(a.refs[i as usize].iter().copied());
                if concrete(&a.prog, i as usize) && !blobs.contains_key(&i) {
                    user_live.push(i);
                    wave.push(i);
                }
            }
        }
        let insts = insts.into_inner().unwrap();
        if let Some(tm) = tm {
            tm.lap("codegen", format!("{} hit / {} compiled ({} generic instances, {} jobs)", rep.fns_hit, rep.fns_compiled, insts.list.len(), self.opts.jobs));
        }
        emit_object(a, &self.ctx, &insts, &user_live, &blobs)
    }

    /// ソースから実行ファイルを作る。前回と同じ .o ならリンクしない
    pub fn build(&mut self, src: &str, output: &std::path::Path) -> R<Report2> {
        let t0 = now_ns();
        let mut tm = Timer { on: self.opts.time, last: t0, start: t0 };
        let mut rep = Report2::default();
        let a = self.analyze(src)?;
        rep.items_parsed = a.n_parsed;
        tm.lap("front", format!("{} items, {} parsed", a.items.len(), a.n_parsed));
        let obj = self.compile_object(&a, &mut rep, Some(&mut tm))?;
        tm.lap("object", format!("{} bytes", obj.len()));
        rep.object_bytes = obj.len();
        let obj_path = output.with_extension("o");
        let same = std::fs::read(&obj_path).is_ok_and(|old| old == obj) && std::fs::read(output).is_ok();
        if same {
            tm.lap("link", "skipped (object unchanged)".to_string());
        } else {
            std::fs::write(&obj_path, &obj).map_err(|e| e.to_string())?;
            let used = crate::link::link(self.opts.linker.as_deref(), obj_path.as_path(), output)?;
            rep.linked = true;
            tm.lap("link", used);
        }
        if self.opts.time {
            eprintln!("  {:<10} {:>8.2} ms", "total", (now_ns() - tm.start) as f64 / 1e6);
        }
        Ok(rep)
    }
}

/// 関数 (インスタンス) 1 つをコンパイルする
fn compile_id(a: &Analysis, insts: &Mutex<Insts>, id: u32) -> R<Blob> {
    compile_id_req(a, insts, id).map(|r| r.0)
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
    acg::compile(&a.prog, insts, id, f, lb, &a.inl)
        .map_err(|e| if acg::has_loc(&e) { e } else { format!("{}:1: in `{}`: {e}", it.line, a.prog.names[body as usize]) })
}

fn emit_object(a: &Analysis, ctx: &CrateCtx, insts: &Insts, user_live: &[u32], blobs: &FxMap<u32, Blob>) -> R<Vec<u8>> {
    let prog = &a.prog;
    let sym_of = |id: u32| -> &str {
        match insts.get(id) {
            Some(i) => &i.sig.symbol,
            None => &prog.sigs[id as usize].symbol,
        }
    };
    // 置く関数: main / export から、再配置と vtable を辿って届くものだけ
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
    let mut placed: Vec<u32> = user_live.iter().copied().filter(|u| reach.contains(u) && blobs.contains_key(u)).collect();
    let mut order: Vec<u32> = (0..insts.list.len() as u32).map(|i| insts.n_fns + i).filter(|i| reach.contains(i) && blobs.contains_key(i)).collect();
    order.sort_by(|a, b| sym_of(*a).cmp(sym_of(*b)));
    // 同じ機械語になるインスタンスは 1 つだけ置く (ICF)
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
            key.extend_from_slice(&(b.code.len() as u32).to_le_bytes());
            key.extend_from_slice(&b.code);
            key.extend_from_slice(format!("{:?}", acg::clif_sig(sig)).as_bytes());
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
                    Target::Sym(ref n) => {
                        key.push(5);
                        key.extend_from_slice(n.as_bytes());
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
    order.retain(|i| !fold.contains_key(i));
    let resolve = |mut f: u32| {
        while let Some(&n) = fold.get(&f) {
            f = n;
        }
        f
    };
    placed.extend(order.iter().copied());
    let fname = |f: u32| -> String {
        let f = resolve(f);
        sym_of(f).to_string()
    };
    // static: コードから参照されているもの (と、その初期値から参照されているもの)
    let mut datas: Vec<DataDef> = vec![];
    let mut anon: HashMap<Vec<u8>, String> = HashMap::new();
    let mut anon_data = |bytes: &[u8], datas: &mut Vec<DataDef>| -> String {
        if let Some(n) = anon.get(&bytes.to_vec()) {
            return n.clone();
        }
        let n = format!(".Lxd{}", anon.len());
        anon.insert(bytes.to_vec(), n.clone());
        datas.push((n.clone(), bytes.to_vec(), vec![], false));
        n
    };
    // (ライブラリの panic の表は `_xT_<名前>` で外に見せる: 実行ファイルの表がそれを指す)
    let static_name = |ci: u32| match (&ctx.lib, prog.consts[ci as usize].name.as_str()) {
        (Some(l), "__EXR_TRAP_TABLE") => format!("_xT_{l}"),
        (None, "__EXR_TRAP_TABLE") | (_, "__EXR_EXE_TABLE") => "_xT__exe".to_string(),
        _ => format!("_xS{}", prog.consts[ci as usize].name),
    };
    let mut todo: Vec<u32> = vec![];
    for u in &placed {
        for r in &blobs[u].relocs {
            if let Target::Static(c) = r.target {
                todo.push(c);
            }
        }
    }
    // (実行ファイルは、依存するクレートが使う unwrap の場所を必ず置く)
    if ctx.lib.is_none() && !ctx.deps.is_empty() {
        todo.extend(prog.const_index.get("__EXR_CALLER").copied());
    }
    todo.sort_unstable();
    todo.dedup();
    let mut done_static: FxSet<u32> = FxSet::default();
    let mut trap_table: Option<u32> = None;
    let mut nested = 0usize;
    while let Some(ci) = todo.pop() {
        if !done_static.insert(ci) {
            continue;
        }
        let c = &prog.consts[ci as usize];
        if c.name == "__EXR_TRAP_TABLE" {
            trap_table = Some(ci);
            continue;
        }
        // (実行ファイルの表: 実行ファイルでは __EXR_TRAP_TABLE と同じもの、ライブラリでは外のもの)
        if c.name == "__EXR_EXE_TABLE" {
            if ctx.lib.is_none() {
                todo.push(trap_table_ci(prog));
            }
            continue;
        }
        // (依存するクレートの static はそちらの .o にある。unwrap の場所 (__EXR_CALLER) は実行ファイルの 1 つを皆で使う)
        if a.is_ext_static(ci) || (ctx.lib.is_some() && c.name == "__EXR_CALLER") {
            continue;
        }
        let dv = crate::ast_consteval::eval_src(prog, &c.expr, c.ty).map_err(|e| format!("in static `{}`: {e}", c.name))?;
        // (入れ子の無名データ (`&[..]` の中身) は名前を付けて置く)
        fn place(
            dv: crate::ast_consteval::DataVal,
            name: String,
            datas: &mut Vec<DataDef>,
            todo: &mut Vec<u32>,
            nested: &mut usize,
            static_name: &dyn Fn(u32) -> String,
        ) {
            let mut rs = vec![];
            for (off, r) in dv.relocs {
                match r {
                    crate::ast_consteval::DRef::Static(c) => {
                        todo.push(c);
                        rs.push((off, static_name(c), 0));
                    }
                    crate::ast_consteval::DRef::Anon(inner) => {
                        *nested += 1;
                        let n = format!(".Lxn{}", *nested);
                        place(inner, n.clone(), datas, todo, nested, static_name);
                        rs.push((off, n, 0));
                    }
                }
            }
            datas.push((name, dv.bytes, rs, false));
        }
        let n0 = datas.len();
        place(dv, static_name(ci), &mut datas, &mut todo, &mut nested, &static_name);
        // (ライブラリの static は外に見せる)
        if (ctx.lib.is_some() && c.item < a.prog.prelude_from && !a.ext_items[c.item as usize]) || (ctx.lib.is_none() && !ctx.deps.is_empty() && c.name == "__EXR_CALLER") {
            if let Some(d) = datas[n0..].iter_mut().find(|d| d.0 == static_name(ci)) {
                d.3 = true;
            }
        }
    }
    // 関数とその再配置
    let mut funcs: Vec<(String, Vec<u8>, Vec<Reloc>, bool)> = vec![];
    let mut vts: HashMap<Vec<u32>, String> = HashMap::new();
    let mut fn_off: Vec<(u32, u64)> = vec![];
    let mut text_end = 0u64;
    for &uid in &placed {
        let blob = &blobs[&uid];
        let data_names: Vec<String> = blob.datas.iter().map(|d| anon_data(d, &mut datas)).collect();
        let mut vt_names = vec![];
        for vt in &blob.vtables {
            if let Some(n) = vts.get(vt) {
                vt_names.push(n.clone());
                continue;
            }
            let n = format!(".Lxv{}", vts.len());
            let mut rs = vec![];
            for (i, &f) in vt.iter().enumerate() {
                if f != u32::MAX && (reach.contains(&resolve(f)) || prog.sigs.get(f as usize).is_some_and(|s| s.is_extern) || a.is_ext(f)) {
                    rs.push((8 * i as u32, fname(f), 0));
                }
            }
            let size: usize = 8 * vt.len().max(1);
            datas.push((n.clone(), vec![0u8; size], rs, false));
            vts.insert(vt.clone(), n.clone());
            vt_names.push(n);
        }
        let relocs: Vec<Reloc> = blob
            .relocs
            .iter()
            .map(|r| Reloc {
                offset: r.offset,
                name: match r.target {
                    Target::Func(t) => fname(t),
                    Target::SelfOffset(_) => sym_of(uid).to_string(),
                    Target::Data(k) => data_names[k as usize].clone(),
                    Target::Static(c) => static_name(c),
                    Target::Vtable(k) => vt_names[k as usize].clone(),
                    Target::Sym(ref n) => n.clone(),
                },
                kind: match r.kind {
                    5 => 9,
                    6 => 2,
                    7 => 1,
                    _ => 4,
                },
                addend: match r.target {
                    Target::SelfOffset(o) => r.addend + o as i64,
                    _ => r.addend,
                },
            })
            .collect();
        let s = &prog.sigs.get(uid as usize);
        let global = insts.get(uid).is_none() && s.is_some_and(|s| s.is_main || s.export);
        text_end = (text_end + 15) / 16 * 16;
        fn_off.push((uid, text_end));
        text_end += blob.code.len() as u64;
        funcs.push((sym_of(uid).to_string(), blob.code.clone(), relocs, global));
    }
    // panic の trap の表 (driver.rs と同じ形式)
    // (依存するクレートのある実行ファイルは、自分が使わなくても表を置く: 依存するクレートの .o が `_xT__exe` を引く)
    if trap_table.is_some() || ctx.lib.is_some() || !ctx.deps.is_empty() {
        let tname = match (trap_table, &ctx.lib) {
            (Some(ci), _) => static_name(ci),
            (None, Some(l)) => format!("_xT_{l}"),
            (None, None) => "_xT__exe".to_string(),
        };
        let mut bytes: Vec<u8> = vec![0; 16];
        let uv = |out: &mut Vec<u8>, mut v: u64| {
            while v >= 0x80 {
                out.push(v as u8 | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        };
        let mut msgs: Vec<Vec<u8>> = vec![];
        let mut fbytes: Vec<u8> = vec![];
        let mut nf = 0u64;
        let (mut prev_fn, mut prev_line) = (0u64, 0i64);
        for &(uid, fo) in &fn_off {
            let blob = &blobs[&uid];
            if blob.traps.is_empty() {
                continue;
            }
            nf += 1;
            // (閉じた依存するクレートの関数の場所は、そのクレートのテキストの中の行: このソースでの位置を足す)
            let shift = a.line_shift(insts, uid) as i64;
            uv(&mut fbytes, fo - prev_fn);
            prev_fn = fo;
            uv(&mut fbytes, blob.traps.len() as u64);
            let mut prev_site = 0u64;
            for &(off, info, mi) in &blob.traps {
                let gm = if mi == u32::MAX {
                    0
                } else {
                    let b = &blob.datas[mi as usize];
                    match msgs.iter().position(|x| x == b) {
                        Some(k) => k as u64 + 1,
                        None => {
                            msgs.push(b.clone());
                            msgs.len() as u64
                        }
                    }
                };
                let (kind, mut line, col) = ((info >> 27) as u64, ((info >> 11) & 0xFFFF) as i64, (info & 0x7FF) as u64);
                if shift > 0 && line > 0 && line < PRELUDE_LINE as i64 {
                    line += shift;
                }
                uv(&mut fbytes, off as u64 - prev_site);
                prev_site = off as u64;
                uv(&mut fbytes, kind | (gm << 4));
                let d = line - prev_line;
                prev_line = line;
                uv(&mut fbytes, ((d << 1) ^ (d >> 63)) as u64);
                uv(&mut fbytes, col);
            }
        }
        // (ファイルの対応: 結合したソースの行 → ファイル。PRELUDE_LINE から後ろは prelude)
        let mut segs: Vec<(u32, String, u32)> = ctx.map.segs.iter().map(|&(st, fi, first)| (st, ctx.map.files.get(fi as usize).cloned().unwrap_or_else(|| "src/main.rs".to_string()), first)).collect();
        if segs.is_empty() {
            segs.push((1, "src/main.rs".to_string(), 1));
        }
        if a.prelude_line0 > 0 && (a.prelude_line0 as u32) < PRELUDE_LINE {
            segs.push((PRELUDE_LINE + 1, "<crustc prelude>".to_string(), 1));
        }
        uv(&mut bytes, segs.len() as u64);
        for (st, n, first) in &segs {
            uv(&mut bytes, *st as u64);
            uv(&mut bytes, *first as u64);
            uv(&mut bytes, n.len() as u64);
            bytes.extend_from_slice(n.as_bytes());
        }
        // (桁のずれ: (行, 桁, ずれ (zigzag)))
        uv(&mut bytes, ctx.map.colfix.len() as u64);
        for &(l, c, d) in &ctx.map.colfix {
            uv(&mut bytes, l as u64);
            uv(&mut bytes, c as u64);
            let d = d as i64;
            uv(&mut bytes, ((d << 1) ^ (d >> 63)) as u64);
        }
        uv(&mut bytes, msgs.len() as u64);
        for mb in &msgs {
            uv(&mut bytes, mb.len() as u64);
            bytes.extend_from_slice(mb);
        }
        uv(&mut bytes, nf);
        bytes.extend_from_slice(&fbytes);
        let mut rs = match funcs.first() {
            Some(f) => vec![(0u32, f.0.clone(), 0i64)],
            None => vec![],
        };
        // (実行ファイル: 8 byte 目に依存するクレートの表の列 [数, アドレス *])
        if ctx.lib.is_none() && !ctx.deps.is_empty() {
            let mut ext = vec![0u8; 8 * (1 + ctx.deps.len())];
            ext[..8].copy_from_slice(&(ctx.deps.len() as u64).to_le_bytes());
            let ers: Vec<(u32, String, i64)> = ctx.deps.iter().enumerate().map(|(i, d)| (8 * (i as u32 + 1), format!("_xT_{d}"), 0i64)).collect();
            datas.push((".Lxext".to_string(), ext, ers, false));
            rs.push((8, ".Lxext".to_string(), 0));
        }
        // (ライブラリの表は 8 byte 目にクレートの印)
        if let Some(l) = &ctx.lib {
            bytes[8..16].copy_from_slice(&(crate_tag(l) as u64).to_le_bytes());
        }
        datas.push((tname, bytes, rs, true));
    }
    Ok(crate::minclif::write_object_full(&funcs, &datas))
}

fn trap_table_ci(prog: &Program) -> u32 {
    prog.const_index.get("__EXR_TRAP_TABLE").copied().unwrap_or(0)
}

/// ワークスペースの 1 つのクレート
#[derive(Clone, Debug)]
pub struct CrateSpec {
    pub name: String,
    /// 根のファイル (lib.rs / main.rs)
    pub root: std::path::PathBuf,
    /// 表示するパス (panic とエラーの場所)
    pub display: String,
    pub lib: bool,
    /// 直接依存するクレートの名前
    pub deps: Vec<String>,
    /// 有効な feature (`features=a,b`)
    pub features: Vec<String>,
}

/// ワークスペースの説明ファイルを読む。1 行に 1 クレート: `lib <名前> <根のファイル> [依存...]` / `bin <名前> <根のファイル> [依存...]`
/// (依存の代わりに `features=a,b` で feature を有効にする)
/// (`#` から後ろはコメント。パスは説明ファイルのディレクトリから)
pub fn read_workspace(path: &std::path::Path) -> R<Vec<CrateSpec>> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut out: Vec<CrateSpec> = vec![];
    for (i, line) in text.lines().enumerate() {
        let line = match line.find('#') {
            Some(k) => &line[..k],
            None => line,
        };
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.is_empty() {
            continue;
        }
        if w.len() < 3 || (w[0] != "lib" && w[0] != "bin") {
            return Err(format!("{}:{}: expected `lib|bin <name> <root> [deps...]`", path.display(), i + 1));
        }
        out.push(CrateSpec {
            name: w[1].to_string(),
            root: dir.join(w[2]),
            display: w[2].to_string(),
            lib: w[0] == "lib",
            deps: w[3..].iter().filter(|s| !s.starts_with("features=")).map(|s| s.to_string()).collect(),
            features: w[3..]
                .iter()
                .filter_map(|s| s.strip_prefix("features="))
                .flat_map(|s| s.split(','))
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect(),
        });
    }
    Ok(out)
}

/// ワークスペースのビルドの結果 (クレートごと)
#[derive(Clone, Debug, Default)]
pub struct WsReport {
    pub crates: Vec<(String, Report2)>,
    /// 書き直した .o の数
    pub objects_written: usize,
    pub linked: usize,
}

/// 依存の順 (依存されるものが先) に並べる
fn topo(crates: &[CrateSpec]) -> R<Vec<usize>> {
    let mut order: Vec<usize> = vec![];
    let mut state = vec![0u8; crates.len()];
    fn visit(i: usize, crates: &[CrateSpec], state: &mut Vec<u8>, order: &mut Vec<usize>) -> R<()> {
        if state[i] == 2 {
            return Ok(());
        }
        if state[i] == 1 {
            return Err(format!("dependency cycle at crate `{}`", crates[i].name));
        }
        state[i] = 1;
        for d in &crates[i].deps {
            let j = crates.iter().position(|c| &c.name == d).ok_or(format!("crate `{}`: unknown dependency `{d}`", crates[i].name))?;
            if !crates[j].lib {
                return Err(format!("crate `{}`: dependency `{d}` is not a lib", crates[i].name));
            }
            visit(j, crates, state, order)?;
        }
        state[i] = 2;
        order.push(i);
        Ok(())
    }
    for i in 0..crates.len() {
        visit(i, crates, &mut state, &mut order)?;
    }
    Ok(order)
}

/// クレートのソース (`pub mod name { .. }`) のうち、使う側のコンパイルに効く部分のハッシュ。
/// 総称でない関数の本体は使う側ではコンパイルしない (シンボルで呼ぶ) のでシグネチャだけを入れる。
/// 総称関数・trait のメンバ・総称の impl のメンバ・`const fn`・`-> impl Trait` は本体も (使う側で実体化・評価する)、
/// 行も入れる (panic の場所)
pub fn interface_hash(text: &str) -> Key {
    let mut h = KeyHasher::new();
    // (`pub mod name {` と最後の `}` の内側を平らにする)
    let body = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => &text[a + 1..b],
        _ => text,
    };
    let flat = lexer::flatten_modules(body);
    let inner: &str = flat.as_deref().unwrap_or(body);
    let items = match lexer::split(inner) {
        Ok(i) => i,
        Err(_) => {
            h.add(text);
            return h.finish();
        }
    };
    for it in &items {
        let t = &inner[it.start..it.end];
        let header_generic = match it.kind {
            ItemKind::ImplMember(hi) => items.get(hi as usize).is_some_and(|hd| inner[hd.start..hd.end].contains('<')),
            ItemKind::TraitMember(_) => true,
            _ => false,
        };
        let sig_end = t.find('{').unwrap_or(t.len());
        let sig = &t[..sig_end];
        let is_fn = sig.contains("fn ");
        let generic_fn = match (sig.find("fn "), sig.find('(')) {
            (Some(a), Some(b)) if a < b => sig[a..b].contains('<'),
            _ => true,
        };
        let concrete = is_fn && !header_generic && !generic_fn && !sig.contains("impl ") && !sig.contains("const fn") && sig_end < t.len();
        if concrete {
            h.add(&1u8);
            h.add(sig);
        } else {
            h.add(&2u8);
            h.add(t);
            if is_fn {
                h.add(&(it.line as u64));
            }
        }
    }
    h.finish()
}

/// 16 進の文字列 (u128::from_str_radix の代わり: crustc の std にはない)
fn parse_hex(s: &str) -> Option<u128> {
    let mut v: u128 = 0;
    for c in s.bytes() {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => return None,
        };
        v = (v << 4) | d as u128;
    }
    Some(v)
}

fn key_hex(k: Key) -> String {
    format!("{:016x}{:016x}", (k >> 64) as u64, k as u64)
}

/// ディレクトリを (親も) 作る (std::fs::create_dir_all の代わり: crustc の std にはない)
fn mkdir_all(dir: &std::path::Path) -> R<()> {
    let s = format!("{}", dir.display());
    let b = s.as_bytes();
    for i in 1..=b.len() {
        if i == b.len() || b[i] == b'/' {
            let p = format!("{}\0", &s[..i]);
            // SAFETY: NUL で終わる文字列
            unsafe { c_mkdir(p.as_bytes().as_ptr(), 0o755) };
        }
    }
    if dir.is_dir() { Ok(()) } else { Err(format!("{}: cannot create the directory", dir.display())) }
}

fn n_lines(s: &str) -> u32 {
    s.as_bytes().iter().filter(|&&c| c == b'\n').count() as u32
}

impl Session2 {
    /// 1 つのクレート (複数のファイル) をビルドする: 根のファイルから `mod x;` を辿る
    pub fn build_path(&mut self, root: &std::path::Path, display: &str, output: &std::path::Path) -> R<Report2> {
        let (src, map) = crate::srcmap::load_crate(root, display, &self.opts.cfg_with(&self.opts.features))?;
        self.ctx = CrateCtx { map: map.clone(), ..Default::default() };
        // (1 ファイルなら場所は `行:桁:` のまま (呼ぶ側がファイル名を付ける))
        let r = self.build(&src, output).map_err(|e| if map.files.len() > 1 { self.ctx.map.relocate_err(&e) } else { e });
        self.ctx = CrateCtx::default();
        r
    }

    /// ワークスペースのクレート i を 1 つコンパイルし、.o・キャッシュ・指紋を書く。(結果, .o が変わったか, 時間 (ns))
    fn build_one(
        &mut self,
        crates: &[CrateSpec],
        loaded: &[Option<(String, crate::srcmap::SrcMap)>],
        dep_loaded: &[Option<(String, crate::srcmap::SrcMap)>],
        prune: (&[Option<Vec<crate::stub::StubItem>>], Option<&[(usize, usize)]>),
        i: usize,
        deps: &[usize],
        fp: &str,
        out_dir: &std::path::Path,
    ) -> R<(Report2, bool, u64)> {
        let c = &crates[i];
        let t1 = now_ns();
        let fp_path = out_dir.join(format!("{}.fp", c.name).as_str());
        let op = out_dir.join(format!("{}.o", c.name).as_str());
        let cp = out_dir.join(format!("{}.crcache2", c.name).as_str());
        if !self.opts.no_cache {
            self.disk = crate::cache2::Cache2::open(cp.as_path());
        }
        // ソース: 自分 + 依存するクレート (後ろに付ける)
        let (own, own_map) = loaded[i].clone().unwrap();
        let mut src = own.clone();
        let mut map = own_map.clone();
        src.push('\n');
        let no_prune = std::env::var_os("CRUSTC_NO_DEP_PRUNE").is_some();
        let mut ranges: Vec<(u32, u32, Vec<usize>, u32)> = vec![];
        for &j in deps {
            let (t0, m) = dep_loaded[j].as_ref().unwrap();
            // (名前で届かないアイテムは空白にする: 平らにする・要約・Program・prelude の選択が使う分だけになる)
            let pruned;
            let t: &str = match (prune.1, prune.0.get(j).and_then(|x| x.as_ref())) {
                (Some(reach), Some(its)) if !no_prune => {
                    let mut keep = vec![false; its.len()];
                    for &(jj, k) in reach {
                        if jj == j {
                            keep[k] = true;
                        }
                    }
                    pruned = crate::stub::prune(t0, its, &keep);
                    pruned.as_str()
                }
                _ => t0.as_str(),
            };
            let at = n_lines(&src) + 1;
            let f0 = map.files.len() as u32;
            for &(s, f, first) in &m.segs {
                map.segs.push((s + at - 1, f + f0, first));
            }
            map.files.extend(m.files.iter().cloned());
            src.push_str(t);
            src.push('\n');
            ranges.push((at, n_lines(&src) + 1, vec![], crate_tag(&crates[j].name)));
        }
        // (依存するクレート j から見えるクレート: j 自身と j の推移的な依存)
        for (k, &j) in deps.iter().enumerate() {
            let mut seen = vec![false; crates.len()];
            let mut st = vec![j];
            seen[j] = true;
            while let Some(x) = st.pop() {
                for d in &crates[x].deps {
                    if let Some(y) = crates.iter().position(|c| &c.name == d) {
                        if !seen[y] {
                            seen[y] = true;
                            st.push(y);
                        }
                    }
                }
            }
            let mut vis: Vec<usize> = vec![];
            for (q, &y) in deps.iter().enumerate() {
                if seen[y] {
                    vis.push(q);
                }
            }
            ranges[k].2 = vis;
        }
        self.ctx = CrateCtx {
            lib: if c.lib { Some(c.name.clone()) } else { None },
            own_lines: n_lines(&own) + 1,
            map: map.clone(),
            deps: deps.iter().map(|&j| crates[j].name.clone()).collect(),
            features: Some(c.features.clone()),
            dep_ranges: ranges,
        };
        let mut r = Report2::default();
        let a = self.analyze(&src).map_err(|e| format!("crate `{}`: {}", c.name, self.ctx.map.relocate_err(&e)))?;
        r.items_parsed = a.n_parsed;
        let obj = self.compile_object(&a, &mut r, None).map_err(|e| format!("crate `{}`: {}", c.name, self.ctx.map.relocate_err(&e)))?;
        r.object_bytes = obj.len();
        if !self.opts.no_cache {
            self.save_cache_keep(cp.as_path())?;
            std::fs::write(fp_path.as_path(), fp).map_err(|e| format!("{}: {e}", fp_path.display()))?;
        }
        let mut changed = false;
        if !std::fs::read(op.as_path()).is_ok_and(|old| old == obj) {
            std::fs::write(op.as_path(), &obj).map_err(|e| format!("{}: {e}", op.display()))?;
            changed = true;
        }
        self.ctx = CrateCtx::default();
        Ok((r, changed, now_ns() - t1))
    }

    /// 細かい指紋 (build_workspace の .fp2): コンパイラ・設定・自分のソース (展開したもの)・依存するクレートの名前と
    /// ファイル、依存するクレートのインターフェースのアイテムのうち、自分の識別子から名前でたどって届くもののハッシュ
    #[allow(clippy::too_many_arguments)]
    fn precise_fp(
        &self,
        c: &CrateSpec,
        own: &str,
        own_map: &crate::srcmap::SrcMap,
        deps: &[usize],
        crates: &[CrateSpec],
        loaded: &[Option<(String, crate::srcmap::SrcMap)>],
        dep_items: &[Option<Vec<crate::stub::StubItem>>],
    ) -> (String, Vec<(usize, usize)>) {
        // (名前 → (クレート, アイテム))
        let mut by_name: FxMap<String, Vec<(usize, usize)>> = FxMap::default();
        for &j in deps {
            for (k, it) in dep_items[j].as_ref().unwrap().iter().enumerate() {
                for nm in &it.names {
                    by_name.entry(nm.clone()).or_default().push((j, k));
                }
            }
        }
        let mut seen_name: FxSet<String> = FxSet::default();
        let mut work: Vec<String> = vec![];
        let b = own.as_bytes();
        for t in lexer::tokenize(b) {
            if t.0 == 0 {
                let w = String::from_utf8_lossy(&b[t.1..t.2]).into_owned();
                if seen_name.insert(w.clone()) {
                    work.push(w);
                }
            }
        }
        let mut reached: Vec<(usize, usize)> = vec![];
        let mut seen_item: FxSet<(usize, usize)> = FxSet::default();
        while let Some(w) = work.pop() {
            let Some(v) = by_name.get(&w) else { continue };
            for &(j, k) in v {
                if !seen_item.insert((j, k)) {
                    continue;
                }
                reached.push((j, k));
                for x in &dep_items[j].as_ref().unwrap()[k].idents {
                    if seen_name.insert(x.clone()) {
                        work.push(x.clone());
                    }
                }
            }
        }
        reached.sort_unstable();
        if std::env::var_os("CRUSTC_DEBUG_REACH").is_some() {
            let mut cnt: FxMap<String, usize> = FxMap::default();
            for &(j, k) in &reached {
                let it = &dep_items[j].as_ref().unwrap()[k];
                *cnt.entry(it.names.first().cloned().unwrap_or_default()).or_default() += 1;
            }
            let mut v: Vec<(usize, String)> = cnt.into_iter().map(|(a, b)| (b, a)).collect();
            v.sort_unstable_by(|a, b| b.cmp(a));
            eprintln!("reach {}: {} items, top {:?}", c.name, reached.len(), &v[..v.len().min(12)]);
        }
        let mut h = KeyHasher::new();
        h.add(&self.cid);
        h.add(&(self.opts.opt as u8));
        h.add(&(c.lib as u8));
        h.add(own);
        for f in &own_map.files {
            h.add(f);
        }
        for &j in deps {
            h.add(&crates[j].name);
            for f in &loaded[j].as_ref().unwrap().1.files {
                h.add(f);
            }
        }
        for &(j, k) in &reached {
            h.add(&(j as u64));
            h.add(&dep_items[j].as_ref().unwrap()[k].hash);
        }
        (key_hex(h.finish()), reached)
    }

    /// ワークスペースをビルドする: クレートごとに .o を out_dir に作り (変わらなければ書かない)、
    /// bin ごとに自分と依存するクレートの .o をリンクする。キャッシュはこの Session2 のものを共有する
    pub fn build_workspace(&mut self, crates: &[CrateSpec], out_dir: &std::path::Path) -> R<WsReport> {
        let t0 = now_ns();
        let order = topo(crates)?;
        mkdir_all(out_dir)?;
        // (読んだクレートのソース: 名前 → (`pub mod 名前 { .. }` にしたテキスト, ファイルの対応))
        let mut loaded: Vec<Option<(String, crate::srcmap::SrcMap)>> = vec![None; crates.len()];
        for &i in &order {
            let c = &crates[i];
            let (text, map) = crate::srcmap::load_crate(c.root.as_path(), &c.display, &self.opts.cfg_with(&c.features))?;
            let text = if c.lib { format!("pub mod {} {{ {text}\n}}\n", c.name) } else { text };
            loaded[i] = Some((text, map));
        }
        let mut rep = WsReport::default();
        let mut changed = vec![false; crates.len()];
        // (推移的な依存: 依存の順)
        let all_deps = |i: usize| -> Vec<usize> {
            let mut seen = vec![false; crates.len()];
            let mut st: Vec<usize> = vec![i];
            while let Some(k) = st.pop() {
                for d in &crates[k].deps {
                    if let Some(j) = crates.iter().position(|c| &c.name == d) {
                        if !seen[j] {
                            seen[j] = true;
                            st.push(j);
                        }
                    }
                }
            }
            order.iter().copied().filter(|&j| seen[j]).collect()
        };
        // (ライブラリの、使う側に効く部分のハッシュ)
        // (ソースが前回と同じなら、前回のものを out_dir/<名前>.iface から読む: `ソースのハッシュ インターフェースのハッシュ`)
        let mut ifaces: Vec<Key> = vec![0; crates.len()];
        for i in 0..crates.len() {
            if !crates[i].lib {
                continue;
            }
            let text = &loaded[i].as_ref().unwrap().0;
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(text);
            let raw = key_hex(h.finish());
            let ip = out_dir.join(format!("{}.iface", crates[i].name).as_str());
            let old = std::fs::read_to_string(ip.as_path()).unwrap_or_default();
            let hit = match old.split_once(' ') {
                Some((r, v)) if r == raw && v.len() == 32 => parse_hex(v),
                _ => None,
            };
            ifaces[i] = match hit {
                Some(v) => v,
                None => {
                    let v = interface_hash(text);
                    let _ = std::fs::write(ip.as_path(), format!("{raw} {}", key_hex(v)));
                    v
                }
            };
        }
        // 古いクレート: (番号, 指紋)
        let mut stale: Vec<(usize, String)> = vec![];
        for &i in &order {
            let c = &crates[i];
            let deps = all_deps(i);
            // 指紋: コンパイラ・設定・自分のソース・依存するクレートのインターフェース。前回と同じで .o があれば何もしない
            let mut h = KeyHasher::new();
            h.add(&self.cid);
            h.add(&(self.opts.opt as u8));
            h.add(&(c.lib as u8));
            h.add(&loaded[i].as_ref().unwrap().0);
            for f in &loaded[i].as_ref().unwrap().1.files {
                h.add(f);
            }
            for &j in &deps {
                h.add(&crates[j].name);
                h.add(&ifaces[j]);
                for f in &loaded[j].as_ref().unwrap().1.files {
                    h.add(f);
                }
            }
            let fp = key_hex(h.finish());
            let fp_path = out_dir.join(format!("{}.fp", c.name).as_str());
            let op = out_dir.join(format!("{}.o", c.name).as_str());
            if !self.opts.no_cache && op.exists() && std::fs::read_to_string(fp_path.as_path()).is_ok_and(|old| old == fp) {
                if self.opts.time {
                    eprintln!("  {:<12} fresh", c.name);
                }
                rep.crates.push((c.name.clone(), Report2::default()));
                continue;
            }
            stale.push((i, fp));
        }
        // 古いクレートが使うソース: 自分は依存するクレートのマクロだけを添えて展開したもの、依存するクレートは
        // 展開して総称でない関数の本体を消したもの (stub::interface_text)。どちらもクレートごとに 1 回だけ作る
        // (使う側で依存するクレートのソース全体を展開・解析し直すと、依存が多いと推移的な依存の大きさに比例する)
        let reuse = std::env::var_os("CRUSTC_NO_DEP_STUB").is_none();
        let mut fp2s: Vec<(usize, String)> = vec![];
        // (古いクレートごとの、依存するクレートのインターフェースのうち名前で届くアイテム: 使う側にはそれだけを渡す)
        let mut reach_of: Vec<Option<Vec<(usize, usize)>>> = vec![None; crates.len()];
        let mut dep_items_all: Vec<Option<Vec<crate::stub::StubItem>>> = (0..crates.len()).map(|_| None).collect();
        let mut own_v: Vec<Option<(String, crate::srcmap::SrcMap)>> = loaded.clone();
        let mut dep_v: Vec<Option<(String, crate::srcmap::SrcMap)>> = loaded.clone();
        if reuse && !stale.is_empty() {
            let t2 = now_ns();
            let mut need = vec![false; crates.len()];
            let mut need_dep = vec![false; crates.len()];
            for &(i, _) in &stale {
                need[i] = true;
                for j in all_deps(i) {
                    need[j] = true;
                    need_dep[j] = true;
                }
            }
            let defs: Vec<String> = (0..crates.len())
                .map(|i| if need[i] { crate::macros::macro_defs(&loaded[i].as_ref().unwrap().0) } else { String::new() })
                .collect();
            for &i in &order {
                if !need[i] {
                    continue;
                }
                let (text, map) = loaded[i].clone().unwrap();
                let mut dd = String::new();
                for j in all_deps(i) {
                    dd.push_str(&defs[j]);
                }
                // (前回と同じ入力なら out_dir/<名前>.prep から読む: `キー\n展開したものの長さ\n展開したもの インターフェース`)
                let mut kh = KeyHasher::new();
                kh.add(&self.cid);
                kh.add(&text);
                kh.add(&dd);
                for f in &crates[i].features {
                    kh.add(f);
                }
                let pkey = key_hex(kh.finish());
                let pp = out_dir.join(format!("{}.prep", crates[i].name).as_str());
                if let Ok(old) = std::fs::read_to_string(pp.as_path()) {
                    if let Some((k, rest)) = old.split_once('\n') {
                        if k == pkey {
                            if let Some((nl, body)) = rest.split_once('\n') {
                                if let Ok(nl) = nl.parse::<usize>() {
                                    if nl <= body.len() && body.is_char_boundary(nl) {
                                        dep_v[i] = Some((body[nl..].to_string(), map.clone()));
                                        own_v[i] = Some((body[..nl].to_string(), map));
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                let marker = "\n// __crustc_dep_macros__\n";
                let exp = if dd.is_empty() && !text.contains("macro_rules!") {
                    text.clone()
                } else {
                    let mut full = text.clone();
                    full.push_str(marker);
                    full.push_str(&dd);
                    let cfg = self.opts.cfg_with(&crates[i].features);
                    match crate::macros::expand_cfg(&full, Some(&cfg)).map_err(|e| format!("crate `{}`: {}", crates[i].name, map.relocate_err(&e)))? {
                        Some(t) => match t.find(marker) {
                            Some(k) => t[..k].to_string(),
                            None => t,
                        },
                        None => text.clone(),
                    }
                };
                let stub = crate::stub::interface_text(&exp);
                if !self.opts.no_cache {
                    let _ = std::fs::write(pp.as_path(), format!("{pkey}\n{}\n{exp}{stub}", exp.len()));
                }
                let _ = need_dep[i];
                dep_v[i] = Some((stub, map.clone()));
                own_v[i] = Some((exp, map));
            }
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms", "(prepare)", (now_ns() - t2) as f64 / 1e6);
            }
            // 細かい指紋: 自分 (展開したもの) と、依存するクレートのインターフェースのうち自分から名前でたどって届くアイテム。
            // 前回と同じなら作り直さない (依存するクレートの使っていない所が変わっただけ: 本体を消してあるので、依存の
            // 依存の関数のシグネチャは、呼んでいる総称関数の本体などから名前が届かなければ効かない)
            let t3 = now_ns();
            let mut dep_items: Vec<Option<Vec<crate::stub::StubItem>>> = (0..crates.len()).map(|_| None).collect();
            let mut keep: Vec<(usize, String)> = vec![];
            for (i, fp) in std::mem::take(&mut stale) {
                let deps = all_deps(i);
                for &j in &deps {
                    if dep_items[j].is_none() {
                        dep_items[j] = Some(crate::stub::items(&dep_v[j].as_ref().unwrap().0));
                    }
                }
                let own = &own_v[i].as_ref().unwrap().0;
                let (fp2, reached) = self.precise_fp(&crates[i], own, &loaded[i].as_ref().unwrap().1, &deps, crates, &loaded, &dep_items);
                let p2 = out_dir.join(format!("{}.fp2", crates[i].name).as_str());
                let op = out_dir.join(format!("{}.o", crates[i].name).as_str());
                if !self.opts.no_cache && op.exists() && std::fs::read_to_string(p2.as_path()).is_ok_and(|old| old == fp2) {
                    let _ = std::fs::write(out_dir.join(format!("{}.fp", crates[i].name).as_str()), &fp);
                    if self.opts.time {
                        eprintln!("  {:<12} fresh (used interface unchanged)", crates[i].name);
                    }
                    rep.crates.push((crates[i].name.clone(), Report2::default()));
                    continue;
                }
                fp2s.push((i, fp2));
                reach_of[i] = Some(reached);
                keep.push((i, fp));
            }
            stale = keep;
            dep_items_all = dep_items;
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms", "(fp2)", (now_ns() - t3) as f64 / 1e6);
            }
        }
        // (クレートは互いのソースしか使わない (.o は使わない) ので、古いものを fork して同時にコンパイルする)
        let par = self.opts.jobs.min(stale.len()).max(1);
        let mut results: Vec<Option<R<(Report2, bool, u64)>>> = vec![None; stale.len()];
        if par > 1 {
            let inner = (self.opts.jobs / par).max(1);
            let mut k0 = 0;
            while k0 < stale.len() {
                let k1 = (k0 + par).min(stale.len());
                let mut kids: Vec<(usize, i32, i32)> = vec![];
                for k in k0..k1 {
                    let mut fds = [0i32; 2];
                    // SAFETY: fds は 2 つの int
                    if unsafe { c_pipe(fds.as_mut_ptr()) } != 0 {
                        continue;
                    }
                    let pid = unsafe { c_fork() };
                    if pid == 0 {
                        unsafe { c_close(fds[0]) };
                        self.opts.jobs = inner;
                        let (i, fp) = (stale[k].0, stale[k].1.clone());
                        self.fresh.clear();
                        let r = self.build_one(crates, &own_v, &dep_v, (&dep_items_all, match &reach_of[i] { Some(v) => Some(v.as_slice()), None => None }), i, &all_deps(i), &fp, out_dir);
                        let mut buf = vec![];
                        match r {
                            Ok((rp, ch, ns)) => {
                                buf.push(0);
                                buf.push(ch as u8);
                                for v in [rp.fns_compiled as u64, rp.fns_hit as u64, rp.items_parsed as u64, rp.object_bytes as u64, ns] {
                                    crate::cache2::put_uv(&mut buf, v);
                                }
                                // (作ったキャッシュのエントリ (要約・関数の機械語) も返す: 後の組のクレートが fork で引き継いで使う。
                                //  prelude の単相化や依存するクレートのアイテムの要約は、多くのクレートで同じ)
                                for &k in &self.fresh {
                                    if let Some(v) = self.mem.get(&k) {
                                        buf.extend_from_slice(&(k as u64).to_le_bytes());
                                        buf.extend_from_slice(&((k >> 64) as u64).to_le_bytes());
                                        crate::cache2::put_uv(&mut buf, v.len() as u64);
                                        buf.extend_from_slice(v);
                                    }
                                }
                            }
                            Err(e) => {
                                buf.push(1);
                                buf.extend_from_slice(e.as_bytes());
                            }
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
                    kids.push((k, pid, fds[0]));
                }
                for (k, pid, fd) in kids {
                    let d = read_all(fd);
                    unsafe { c_close(fd) };
                    let mut st = 0i32;
                    unsafe { c_waitpid(pid, &mut st, 0) };
                    let mut r = crate::cache2::Rd { d: &d, pos: 0 };
                    results[k] = match r.u8() {
                        Some(0) => {
                            let ch = r.u8().unwrap_or(1) != 0;
                            let mut v = [0u64; 5];
                            for x in v.iter_mut() {
                                *x = r.uv().unwrap_or(0);
                            }
                            let rp = Report2 { fns_compiled: v[0] as usize, fns_hit: v[1] as usize, items_parsed: v[2] as usize, object_bytes: v[3] as usize, linked: false };
                            while r.pos < d.len() {
                                let (Some(lo), Some(hi), Some(n)) = (r.u64(), r.u64(), r.uv()) else { break };
                                let Some(body) = r.take(n as usize) else { break };
                                let k = (lo as u128) | ((hi as u128) << 64);
                                if !self.mem.contains_key(&k) {
                                    self.mem.insert(k, body.to_vec());
                                }
                            }
                            Some(Ok((rp, ch, v[4])))
                        }
                        Some(_) => Some(Err(String::from_utf8_lossy(&d[1..]).into_owned())),
                        // (子が落ちた: ここでもう一度 (エラーならそれが出る))
                        None => None,
                    };
                }
                k0 = k1;
            }
        }
        for k in 0..stale.len() {
            let (i, fp) = (stale[k].0, stale[k].1.clone());
            let r = match results[k].take() {
                Some(r) => r,
                None => self.build_one(crates, &own_v, &dep_v, (&dep_items_all, match &reach_of[i] { Some(v) => Some(v.as_slice()), None => None }), i, &all_deps(i), &fp, out_dir),
            };
            let (r, ch, ns) = r?;
            if !self.opts.no_cache {
                if let Some((_, f2)) = fp2s.iter().find(|x| x.0 == i) {
                    let _ = std::fs::write(out_dir.join(format!("{}.fp2", crates[i].name).as_str()), f2);
                }
            }
            changed[i] = ch;
            if ch {
                rep.objects_written += 1;
            }
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms  {} fns compiled, {} hit, {} items parsed{}", crates[i].name, ns as f64 / 1e6, r.fns_compiled, r.fns_hit, r.items_parsed, if ch { "" } else { " (object unchanged)" });
            }
            rep.crates.push((crates[i].name.clone(), r));
        }
        for &i in &order {
            let c = &crates[i];
            if c.lib {
                continue;
            }
            let deps = all_deps(i);
            let exe = out_dir.join(c.name.as_str());
            if !changed[i] && !deps.iter().any(|&j| changed[j]) && exe.exists() {
                continue;
            }
            let t1 = now_ns();
            let mut objs: Vec<std::path::PathBuf> = vec![out_dir.join(format!("{}.o", c.name).as_str())];
            objs.extend(deps.iter().map(|&j| out_dir.join(format!("{}.o", crates[j].name).as_str())));
            let refs: Vec<&std::path::Path> = objs.iter().map(|p| p.as_path()).collect();
            crate::link::link_many(self.opts.linker.as_deref(), &refs, exe.as_path())?;
            rep.linked += 1;
            if self.opts.time {
                eprintln!("  {:<12} {:>8.2} ms  link {} objects", format!("link {}", c.name), (now_ns() - t1) as f64 / 1e6, objs.len());
            }
        }
        self.ctx = CrateCtx::default();
        if self.opts.time {
            eprintln!("  {:<12} {:>8.2} ms", "total", (now_ns() - t0) as f64 / 1e6);
        }
        Ok(rep)
    }
}

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
        let (mut n_keys, mut n_hit, mut n_checked) = (0, 0, 0);
        let insts = Mutex::new(Insts::new(&a.prog));
        let mut errs: Vec<String> = vec![];
        let mut done: FxSet<u32> = FxSet::default();
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
                let k = self.key_of(&a, &insts, id);
                let kc = k ^ CHECK_SALT;
                n_keys += 1;
                // (キャッシュにあれば検査済み: 読み戻すと要求していたインスタンスも要求し直す)
                if self.get(k).and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)).is_some() {
                    continue;
                }
                if check_only && self.get(kc).and_then(|d| crate::cache2::decode_blob(&d, &a.prog, &insts)).is_some() {
                    n_hit += 1;
                    continue;
                }
                n_checked += 1;
                match compile_id_req(&a, &insts, id) {
                    Ok((b, req)) => {
                        if !self.opts.no_cache {
                            let b = if check_only {
                                let relocs = req.into_iter().map(|i| acg::BlobReloc { offset: 0, kind: 4, addend: 0, target: acg::Target::Func(i) }).collect();
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
                    }
                    Err(e) => errs.push(e),
                }
            }
            wave = std::mem::take(&mut insts.lock().unwrap().pending);
            wave.retain(|id| !done.contains(id));
        }
        tm.lap("check", format!("{n_keys} keys, {n_hit} cached, {n_checked} checked"));
        let mut d: Vec<Diagnostic> = errs.iter().map(|e| Diagnostic::parse(e)).filter(|d| d.line <= user_lines).collect();
        d.sort_by(|x, y| (x.line, x.col).cmp(&(y.line, y.col)));
        d.dedup();
        d
    }
}
