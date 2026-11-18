//! 最小の LSP サーバ (stdio)。対応: initialize / didOpen / didChange (全文同期) / didClose /
//! shutdown / exit / textDocument/documentSymbol / workspace/symbol / textDocument/definition / textDocument/references /
//! textDocument/hover / textDocument/completion。
//! 開く・変更するたびに [`Session2::check`] を走らせて publishDiagnostics を返す。
//!
//! ドキュメントごとに Session2 を 1 つ持ち、キャッシュはメモリ上だけに置く。
//! 変わっていない関数は「検査済み」の印で飛ばすので、応答時間は変更量に比例する。
//! シンボルは crustc_ide::symbols で字句の列から読む (構文が壊れていても読める所まで)。workspace/symbol・定義・参照は
//! 開いているドキュメントと、ワークスペースの根の下の .rs ファイル (更新時刻と大きさで覚える) の索引
//! (crustc_ide::navigate::FileIndex) から引く。ワークスペースの本文と字句の列は `CRUSTC_LSP_MEM_MB` (既定 4) MB の分だけ
//! 持ち、超えた分のファイルは定義と名前の集合だけにする (参照・hover で要るときに読み直し、同じだけの大きさの LRU で覚える)。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crustc_core::driver2::{Options2, Session2};
use crustc_ide::navigate::{self, FileIndex, Loc};
use crustc_ide::symbols::{self, LineIndex, Symbol};

/// 開いているドキュメント
struct Doc {
    s: Session2,
    /// 本文と、それに対する診断・シンボル (シンボルは求められたときに作る)
    text: String,
    diags: Vec<Value>,
    syms: Option<Vec<Symbol>>,
    /// 定義・参照・workspace/symbol の索引 (求められたときに作る)
    idx: Option<FileIndex>,
}

/// ワークスペースのファイル (開いていないもの) の索引と、作ったときの (更新時刻, 大きさ)
struct WsFile {
    stamp: (u64, u64),
    idx: FileIndex,
}

pub struct Server {
    docs: HashMap<String, Doc>,
    /// ワークスペースの根 (initialize の rootUri / rootPath / workspaceFolders)
    root: Option<PathBuf>,
    /// ワークスペースのファイル (開いていないもの) のシンボル
    files: HashMap<PathBuf, WsFile>,
    /// std / core / alloc の定義 (initialize の後にスレッドで作る。定義・参照で使う)
    lib_job: Option<std::thread::JoinHandle<Vec<(String, FileIndex)>>>,
    lib: Vec<(String, FileIndex)>,
    /// ワークスペースのファイルの並び (パス, モジュール, クレート, 本体か)。ドキュメントの変更・ファイルの変更で作り直す
    layout: Option<Vec<(PathBuf, Vec<String>, u32, bool)>>,
    /// クレートの番号 → 有効な feature (Cargo.toml の default から)。cfg で外れたアイテムを除くのに使う
    feats: HashMap<u32, Vec<String>>,
    /// 本文を持つワークスペースのファイルの大きさの上限 (byte)。超えた分は slim にする
    budget: usize,
    /// slim なファイルを読み直したもの (uri, 索引)。新しいものが後ろ。本文の合計が budget を超えたら古いものから捨てる
    cache: RefCell<Vec<(String, Rc<FileIndex>)>>,
    /// クレートの名前 (workspace/symbol に出す): ワークスペースのパッケージと std のクレート (uri は本体の根のファイル)
    crates: Vec<(String, String)>,
    pub exit: bool,
    shutdown: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Server {
        Server {
            docs: HashMap::new(),
            root: None,
            files: HashMap::new(),
            lib_job: None,
            lib: vec![],
            layout: None,
            feats: HashMap::new(),
            budget: std::env::var("CRUSTC_LSP_MEM_MB").ok().and_then(|v| v.parse::<f64>().ok()).map(|m| (m * 1048576.0) as usize).unwrap_or(4 << 20),
            cache: RefCell::new(vec![]),
            crates: vec![],
            exit: false,
            shutdown: false,
        }
    }

    /// 本文を持つワークスペースのファイルの大きさの上限 (byte。既定は `CRUSTC_LSP_MEM_MB`、なければ 4 MB)
    pub fn set_mem_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }

    /// 1 メッセージを処理し、送り返すメッセージ (JSON 文字列) を返す
    pub fn handle(&mut self, msg: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(msg) else {
            return vec![json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}).to_string()];
        };
        let method = v["method"].as_str().unwrap_or("");
        let id = v.get("id").cloned();
        let p = &v["params"];
        let reply =
            |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
        match method {
            "initialize" => {
                let root = p["rootUri"]
                    .as_str()
                    .and_then(uri_path)
                    .or_else(|| p["rootPath"].as_str().map(PathBuf::from))
                    .or_else(|| p["workspaceFolders"][0]["uri"].as_str().and_then(uri_path));
                self.root = root;
                if self.lib_job.is_none() && std::env::var_os("CRUSTC_LSP_NO_STD").is_none() {
                    self.lib_job = Some(std::thread::spawn(index_std));
                }
                vec![reply(json!({
                    "capabilities": { "textDocumentSync": 1, "documentSymbolProvider": true, "workspaceSymbolProvider": true, "definitionProvider": true, "referencesProvider": true,
                        "hoverProvider": true, "completionProvider": { "triggerCharacters": [".", ":"] },
                        "renameProvider": { "prepareProvider": true }, "signatureHelpProvider": { "triggerCharacters": ["(", ","] }, "documentHighlightProvider": true },
                    "serverInfo": { "name": "crustc", "version": env!("CARGO_PKG_VERSION") }
                }))]
            }
            "textDocument/documentSymbol" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                vec![reply(self.document_symbols(&uri))]
            }
            "workspace/symbol" => {
                let q = p["query"].as_str().unwrap_or("").to_string();
                vec![reply(self.workspace_symbols(&q))]
            }
            "textDocument/definition" | "textDocument/references" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                let (line, ch) = (p["position"]["line"].as_u64().unwrap_or(0) as u32, p["position"]["character"].as_u64().unwrap_or(0) as u32);
                let refs = method == "textDocument/references";
                let decl = p["context"]["includeDeclaration"].as_bool().unwrap_or(true);
                vec![reply(self.navigate(&uri, line, ch, refs, decl))]
            }
            "textDocument/hover" | "textDocument/completion" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                let (line, ch) = (p["position"]["line"].as_u64().unwrap_or(0) as u32, p["position"]["character"].as_u64().unwrap_or(0) as u32);
                let r = if method == "textDocument/hover" { self.hover(&uri, line, ch) } else { self.complete(&uri, line, ch) };
                vec![reply(r)]
            }
            "textDocument/documentHighlight" | "textDocument/signatureHelp" | "textDocument/prepareRename" | "textDocument/rename" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                let (line, ch) = (p["position"]["line"].as_u64().unwrap_or(0) as u32, p["position"]["character"].as_u64().unwrap_or(0) as u32);
                let r = match method {
                    "textDocument/documentHighlight" => Ok(self.highlight(&uri, line, ch)),
                    "textDocument/signatureHelp" => Ok(self.signature_help(&uri, line, ch)),
                    "textDocument/prepareRename" => self.prepare_rename(&uri, line, ch),
                    _ => self.rename(&uri, line, ch, p["newName"].as_str().unwrap_or("")),
                };
                vec![match r {
                    Ok(v) => reply(v),
                    // (RequestFailed: 理由をエディタに出す)
                    Err(m) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32803, "message": m}}).to_string(),
                }]
            }
            "shutdown" => {
                self.shutdown = true;
                vec![reply(Value::Null)]
            }
            "exit" => {
                self.exit = true;
                vec![]
            }
            "textDocument/didOpen" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                let text = p["textDocument"]["text"].as_str().unwrap_or("");
                vec![self.diagnose(uri, text, p["textDocument"]["version"].clone())]
            }
            "textDocument/didChange" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                // 全文同期 (textDocumentSync = 1) なので最後の変更が全文
                let Some(text) = p["contentChanges"]
                    .as_array()
                    .and_then(|a| a.last())
                    .and_then(|c| c["text"].as_str())
                else {
                    return vec![];
                };
                vec![self.diagnose(uri, text, p["textDocument"]["version"].clone())]
            }
            "textDocument/didClose" => {
                let uri = p["textDocument"]["uri"].as_str().unwrap_or("").to_string();
                self.docs.remove(&uri);
                vec![publish(&uri, Value::Null, vec![])]
            }
            _ if id.is_some() => vec![
                json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("method not found: {method}")}
                })
                .to_string(),
            ],
            _ => vec![], // 未対応の通知は無視
        }
    }

    fn diagnose(&mut self, uri: String, text: &str, version: Value) -> String {
        let doc = self
            .docs
            .entry(uri.clone())
            .or_insert_with(|| Doc { s: Session2::new(Options2 { jobs: 1, ..Default::default() }), text: String::new(), diags: vec![], syms: None, idx: None });
        // 本文が同じ (保存しただけ等) なら前回の診断をそのまま返す。比較は memcmp なのでハッシュより安い
        if !doc.text.is_empty() && doc.text == text {
            return publish(&uri, version, doc.diags.clone());
        }
        let diags: Vec<Value> = doc
            .s
            .check(text)
            .into_iter()
            .map(|d| {
                let (l, c) = (d.line.saturating_sub(1), d.col.saturating_sub(1));
                json!({
                    "range": { "start": {"line": l, "character": c}, "end": {"line": l, "character": c + 1} },
                    "severity": 1,
                    "source": "crustc",
                    "message": d.msg,
                })
            })
            .collect();
        doc.text.clear();
        doc.text.push_str(text);
        doc.diags = diags.clone();
        doc.syms = None;
        doc.idx = None;
        self.layout = None;
        publish(&uri, version, diags)
    }

    /// textDocument/documentSymbol: シンボルの木 (DocumentSymbol)。開いていなければファイルを読む
    fn document_symbols(&mut self, uri: &str) -> Value {
        let (text, syms) = match self.docs.get_mut(uri) {
            Some(d) => {
                if d.syms.is_none() {
                    d.syms = Some(symbols::document_symbols(&d.text));
                }
                (d.text.clone(), d.syms.clone().unwrap_or_default())
            }
            None => {
                let Some(text) = uri_path(uri).and_then(|p| std::fs::read_to_string(p).ok()) else { return json!([]) };
                let syms = symbols::document_symbols(&text);
                (text, syms)
            }
        };
        let li = LineIndex::new(&text);
        fn conv(v: &[Symbol], text: &str, li: &LineIndex) -> Vec<Value> {
            v.iter()
                .map(|s| {
                    let mut o = json!({
                        "name": s.name,
                        "kind": s.lsp_kind(),
                        "range": range(text, li, s.start, s.end),
                        "selectionRange": range(text, li, s.name_start, s.name_end),
                    });
                    if !s.detail.is_empty() {
                        o["detail"] = json!(s.detail);
                    }
                    if !s.children.is_empty() {
                        o["children"] = json!(conv(&s.children, text, li));
                    }
                    o
                })
                .collect()
        }
        json!(conv(&syms, &text, &li))
    }

    /// ドキュメントとワークスペースのファイルの索引を新しくし、(uri, 索引) の列を返す (開いているものが先)
    fn workspace(&mut self, with_lib: bool) -> Vec<(String, &FileIndex)> {
        self.refresh(with_lib);
        self.view(with_lib)
    }

    /// 開いているドキュメント → ワークスペースのファイル (パスの順) → (with_lib なら) std の索引
    fn view(&self, with_lib: bool) -> Vec<(String, &FileIndex)> {
        let mut out: Vec<(String, &FileIndex)> = vec![];
        for (uri, d) in &self.docs {
            if let Some(ix) = d.idx.as_ref() {
                out.push((uri.clone(), ix));
            }
        }
        let mut fs: Vec<(&PathBuf, &WsFile)> = self.files.iter().collect();
        fs.sort_by(|a, b| a.0.cmp(b.0));
        for (p, f) in fs {
            out.push((path_uri(p), &f.idx));
        }
        if with_lib {
            for (u, f) in &self.lib {
                out.push((u.clone(), f));
            }
        }
        out
    }

    /// ws[i] が slim なら本文から作り直す (LRU で覚える)
    fn load(&self, ws: &[(String, &FileIndex)], i: usize) -> Option<Rc<FileIndex>> {
        let uri = &ws.get(i)?.0;
        let mut c = self.cache.borrow_mut();
        if let Some(j) = c.iter().position(|x| &x.0 == uri) {
            let e = c.remove(j);
            let r = e.1.clone();
            c.push(e);
            return Some(r);
        }
        let text = std::fs::read_to_string(uri_path(uri)?).ok()?;
        let r = Rc::new(ws[i].1.reload(text));
        c.push((uri.clone(), r.clone()));
        let mut total: usize = c.iter().map(|x| x.1.text_len()).sum();
        while total > self.budget && c.len() > 1 {
            total -= c[0].1.text_len();
            c.remove(0);
        }
        Some(r)
    }

    fn refresh(&mut self, with_lib: bool) {
        if with_lib {
            if let Some(j) = self.lib_job.take() {
                self.lib = j.join().unwrap_or_default();
            }
        }
        let root = self.root.clone();
        let module = |p: &Path| -> Vec<String> {
            match root.as_ref().and_then(|r| p.strip_prefix(r).ok()) {
                Some(rel) => navigate::module_of(&rel.to_string_lossy()),
                None => vec![],
            }
        };
        // (Cargo のワークスペースならそのモジュールの木のファイル、なければ根の下の .rs 全部)
        if self.layout.is_none() {
            self.layout = Some(match root.as_ref().map(|r| (r, cargo_files(r))) {
                Some((_, Some((v, feats, names)))) => {
                    self.feats = feats;
                    // (クレートの名前: パッケージと、std のクレート (場所は最初のパッケージの根: rust-analyzer と同じ))
                    self.crates = names.iter().map(|(n, p)| (n.clone(), path_uri(p))).collect();
                    if let Some((_, p)) = names.first() {
                        for n in ["core", "alloc", "std", "test", "proc_macro"] {
                            self.crates.push((n.to_string(), path_uri(p)));
                        }
                    }
                    v
                }
                Some((r, None)) => {
                    let mut ps = vec![];
                    rs_files(r, &mut ps, 0);
                    ps.into_iter().map(|p| (p.clone(), module(&p), 0, in_src(Some(r), &p))).collect()
                }
                None => vec![],
            });
        }
        let layout = self.layout.clone().unwrap_or_default();
        let place: HashMap<&PathBuf, (&Vec<String>, u32, bool)> = layout.iter().map(|x| (&x.0, (&x.1, x.2, x.3))).collect();
        let mut open: Vec<PathBuf> = vec![];
        for (uri, d) in self.docs.iter_mut() {
            let p = uri_path(uri).unwrap_or_default();
            if d.idx.is_none() {
                let (m, kr, main) = match place.get(&p) {
                    Some(&(m, kr, main)) => (m.clone(), kr, main),
                    None => (module(&p), 0, in_src(root.as_deref(), &p)),
                };
                let mut ix = FileIndex::new(d.text.clone(), m);
                ix.in_src = main;
                ix.krate = kr;
                if let Some(fs) = self.feats.get(&kr) {
                    ix.apply_cfg(fs.clone());
                }
                d.idx = Some(ix);
            }
            open.push(p);
        }
        if root.is_some() {
            let paths = layout.clone();
            let mut changed = false;
            // (本文を持つファイルの大きさの合計: budget を超えたら、そこからのファイルは slim)
            let mut full: usize = self.files.values().filter(|f| !f.idx.slim).map(|f| f.idx.text_len()).sum();
            let mut keep: HashMap<PathBuf, WsFile> = HashMap::new();
            for (p, pm, kr, main) in paths {
                if open.contains(&p) {
                    continue;
                }
                let Ok(md) = std::fs::metadata(&p) else { continue };
                let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
                let stamp = (mtime, md.len());
                let f = match self.files.remove(&p) {
                    Some(f) if f.stamp == stamp => f,
                    _ => {
                        // (変わったファイル: mod の宣言も変わったかもしれないので、次は並びを作り直す)
                        if self.files.contains_key(&p) || !self.files.is_empty() {
                            changed = true;
                        }
                        let text = if md.len() <= 8 << 20 { std::fs::read_to_string(&p).unwrap_or_default() } else { String::new() };
                        let mut ix = FileIndex::new(text, pm.clone());
                        ix.in_src = main;
                        ix.krate = kr;
                        if let Some(fs) = self.feats.get(&kr) {
                            ix.apply_cfg(fs.clone());
                        }
                        if full + ix.text_len() > self.budget {
                            ix.make_slim();
                        } else {
                            full += ix.text_len();
                        }
                        // (前に読み直したものは古い)
                        let u = path_uri(&p);
                        self.cache.get_mut().retain(|x| x.0 != u);
                        WsFile { stamp, idx: ix }
                    }
                };
                keep.insert(p, f);
            }
            self.files = keep;
            if changed {
                self.layout = None;
            }
        }
    }

    /// workspace/symbol: 名前が問い合わせに合うシンボル (SymbolInformation)。合い方の良い順に 256 個まで
    /// workspace/symbol: rust-analyzer と同じ決まりで
    /// - 問い合わせの文字が順に (間を空けてよい) 名前に出てくるもの。大文字を含む問い合わせは大文字と小文字を区別する
    /// - まず型 (struct / enum / union / trait / type と、クレートの名前) だけ。なければ全部 (フィールド・impl と、
    ///   trait の impl の中のメンバは除く)
    /// - use の別名 (`use a::B as C` の C) と再エクスポート (`pub use a::B`) も入れる
    /// - 名前 (小文字にしたもの) の順に 128 個まで
    fn workspace_symbols(&mut self, q: &str) -> Value {
        // (std は入れない: rust-analyzer の既定と同じくワークスペースだけ)
        let ws = self.workspace(false);
        let case = q.chars().any(|c| c.is_uppercase());
        let matches = |name: &str| -> bool {
            let mut it = name.chars();
            q.chars().all(|c| it.any(|x| if case { x == c } else { x.to_lowercase().eq(c.to_lowercase()) }))
        };
        let is_type = |k: u32| matches!(k, 23 | 10 | 11 | 26 | 4);
        // (候補: (名前, 種類, uri, 範囲, 囲むもの))
        let mut hits: Vec<(String, u32, String, [u32; 4], String)> = vec![];
        // (名前 → 種類: use の別名・再エクスポートの種類に)
        let mut kind_of: HashMap<&str, u32> = HashMap::new();
        for (_, f) in &ws {
            for d in &f.defs {
                if d.owner.is_empty() {
                    kind_of.entry(d.name.as_str()).or_insert(d.kind.lsp());
                }
            }
        }
        for (uri, f) in &ws {
            let mut li: Option<LineIndex> = None;
            for d in &f.defs {
                if matches!(d.kind, symbols::SymKind::Impl | symbols::SymKind::Field) || (d.container.starts_with("impl") && d.container.contains(" for ")) || !matches(&d.name) {
                    continue;
                }
                let ((l0, c0), (l1, c1)) = ends(f, &mut li, d.start, d.end);
                let c = if d.container.starts_with("impl") { d.owner.clone() } else { d.container.to_string() };
                hits.push((d.name.clone(), d.kind.lsp(), uri.clone(), [l0, c0, l1, c1], c));
            }
            for (name, l, c, n) in &f.use_syms {
                if matches(name) {
                    hits.push((name.clone(), kind_of.get(name.as_str()).copied().unwrap_or(13), uri.clone(), [*l, *c, *l, c + n], String::new()));
                }
            }
        }
        for (name, uri) in &self.crates {
            if matches(name) {
                hits.push((name.clone(), 4, uri.clone(), [0, 0, 0, 0], String::new()));
            }
        }
        if hits.iter().any(|h| is_type(h.1)) {
            hits.retain(|h| is_type(h.1));
        }
        hits.sort_by(|a, b| (a.0.to_lowercase(), &a.0, &a.2, a.3).cmp(&(b.0.to_lowercase(), &b.0, &b.2, b.3)));
        hits.truncate(128);
        json!(hits
            .into_iter()
            .map(|(name, kind, uri, r, c)| {
                let mut o = json!({
                    "name": name,
                    "kind": kind,
                    "location": { "uri": uri, "range": { "start": {"line": r[0], "character": r[1]}, "end": {"line": r[2], "character": r[3]} } },
                });
                if !c.is_empty() {
                    o["containerName"] = json!(c);
                }
                o
            })
            .collect::<Vec<_>>())
    }

    /// uri の (line, ch) で f を呼ぶ: (サーバ, ws, 索引の列, ファイルの番号, byte の位置)
    fn at<R>(&mut self, uri: &str, line: u32, ch: u32, f: impl FnOnce(&Self, &[(String, &FileIndex)], &[&FileIndex], usize, usize) -> R) -> Option<R> {
        self.refresh(true);
        let ws = self.view(true);
        // (開いているドキュメントか、ワークスペースのファイル。uri の書き方の違いはパスで比べる)
        let want = uri_path(uri);
        let fi = ws.iter().position(|(u, _)| u == uri || (want.is_some() && uri_path(u) == want))?;
        let held = if ws[fi].1.slim { Some(self.load(&ws, fi)?) } else { None };
        let mut idx: Vec<&FileIndex> = ws.iter().map(|x| x.1).collect();
        if let Some(h) = held.as_ref() {
            idx[fi] = h;
        }
        let off = offset(&idx[fi].text, line, ch)?;
        Some(f(self, &ws, &idx, fi, off))
    }

    /// textDocument/definition (refs = false) と textDocument/references (refs = true)
    fn navigate(&mut self, uri: &str, line: u32, ch: u32, refs: bool, decl: bool) -> Value {
        self.at(uri, line, ch, |srv, ws, idx, fi, off| {
            let load = |i: usize| srv.load(ws, i);
            let locs: Vec<Loc> = if refs { navigate::references(idx, fi, off, decl, &load) } else { navigate::definition(idx, fi, off) };
            let out: Vec<Value> = srv.ranges(ws, idx, &locs).into_iter().map(|(u, r)| json!({ "uri": u, "range": r })).collect();
            if refs || out.len() != 1 { json!(out) } else { out.into_iter().next().unwrap() }
        })
        .unwrap_or(Value::Null)
    }

    /// 位置の列 → (uri, LSP の範囲)。slim なファイルの中の位置は、読み直した本文で行と桁にする (定義の名前の位置は索引が覚えている)
    fn ranges(&self, ws: &[(String, &FileIndex)], idx: &[&FileIndex], locs: &[Loc]) -> Vec<(String, Value)> {
        let mut lis: HashMap<usize, Option<LineIndex>> = HashMap::new();
        let mut held: HashMap<usize, Rc<FileIndex>> = HashMap::new();
        locs.iter()
            .map(|l| {
                let mut f = idx[l.file];
                if f.slim && !f.lib && f.lib_pos(l.start).is_none() {
                    if !held.contains_key(&l.file) {
                        if let Some(r) = self.load(ws, l.file) {
                            held.insert(l.file, r);
                        }
                    }
                    if let Some(r) = held.get(&l.file) {
                        f = r;
                    }
                }
                let ((l0, c0), (l1, c1)) = ends(f, lis.entry(l.file).or_default(), l.start, l.end);
                (ws[l.file].0.clone(), json!({ "start": {"line": l0, "character": c0}, "end": {"line": l1, "character": c1} }))
            })
            .collect()
    }

    /// textDocument/documentHighlight: 同じファイルの中の参照
    fn highlight(&mut self, uri: &str, line: u32, ch: u32) -> Value {
        self.at(uri, line, ch, |srv, ws, idx, fi, off| {
            let load = |i: usize| srv.load(ws, i);
            let locs: Vec<Loc> = navigate::highlights(idx, fi, off, &load);
            json!(srv.ranges(ws, idx, &locs).into_iter().map(|(_, r)| json!({ "range": r, "kind": 1 })).collect::<Vec<_>>())
        })
        .unwrap_or(Value::Null)
    }

    /// textDocument/prepareRename (Err は理由)
    fn prepare_rename(&mut self, uri: &str, line: u32, ch: u32) -> Result<Value, String> {
        self.at(uri, line, ch, |srv, ws, idx, fi, off| {
            let l = navigate::prepare_rename(idx, fi, off)?;
            let (_, r) = srv.ranges(ws, idx, &[l]).pop().unwrap();
            Ok(json!({ "range": r, "placeholder": idx[fi].text[l.start..l.end] }))
        })
        .unwrap_or(Err("ファイルが分からない".into()))
    }

    /// textDocument/rename → WorkspaceEdit (Err は理由)
    fn rename(&mut self, uri: &str, line: u32, ch: u32, new: &str) -> Result<Value, String> {
        self.at(uri, line, ch, |srv, ws, idx, fi, off| {
            let load = |i: usize| srv.load(ws, i);
            let edits = navigate::rename(idx, fi, off, new, &load)?;
            let locs: Vec<Loc> = edits.iter().map(|e| e.0).collect();
            let mut changes = serde_json::Map::new();
            for ((u, r), (_, txt)) in srv.ranges(ws, idx, &locs).into_iter().zip(edits) {
                changes.entry(u).or_insert_with(|| json!([])).as_array_mut().unwrap().push(json!({ "range": r, "newText": txt }));
            }
            Ok(json!({ "changes": changes }))
        })
        .unwrap_or(Err("ファイルが分からない".into()))
    }

    /// textDocument/signatureHelp
    fn signature_help(&mut self, uri: &str, line: u32, ch: u32) -> Value {
        self.at(uri, line, ch, |srv, ws, idx, fi, off| {
            let load = |i: usize| srv.load(ws, i);
            let Some((sig, params, active, doc)) = navigate::signature_help(idx, fi, off, &load) else { return Value::Null };
            // (引数の範囲は UTF-16 の桁で)
            let u16 = |b: usize| sig[..b].encode_utf16().count();
            let ps: Vec<Value> = params.iter().map(|&(a, e)| json!({ "label": [u16(a), u16(e)] })).collect();
            let mut sg = json!({ "label": sig, "parameters": ps, "activeParameter": active });
            if !doc.is_empty() {
                sg["documentation"] = json!({ "kind": "markdown", "value": doc });
            }
            json!({ "signatures": [sg], "activeSignature": 0, "activeParameter": active })
        })
        .unwrap_or(Value::Null)
    }

    /// textDocument/hover: 定義のシグネチャ (markdown)
    fn hover(&mut self, uri: &str, line: u32, ch: u32) -> Value {
        self.at(uri, line, ch, |srv, ws, idx, fi, off| {
            let load = |i: usize| srv.load(ws, i);
            match navigate::hover(idx, fi, off, &load) {
                Some(md) => json!({ "contents": { "kind": "markdown", "value": md } }),
                None => Value::Null,
            }
        })
        .unwrap_or(Value::Null)
    }

    /// textDocument/completion
    fn complete(&mut self, uri: &str, line: u32, ch: u32) -> Value {
        self.at(uri, line, ch, |_, _, idx, fi, off| {
            let (items, cut) = navigate::complete(idx, fi, off);
            let items: Vec<Value> = items
                .into_iter()
                .map(|it| {
                    let kind = match (it.local, it.kind) {
                        (true, _) => 6,
                        (_, Some(k)) => completion_kind(k),
                        (_, None) if it.detail == "keyword" => 14,
                        _ => 7,
                    };
                    let mut o = json!({ "label": it.label, "kind": kind });
                    if !it.detail.is_empty() {
                        o["detail"] = json!(it.detail);
                    }
                    o
                })
                .collect();
            json!({ "isIncomplete": cut, "items": items })
        })
        .unwrap_or(Value::Null)
    }
}

/// シンボルの種類 → LSP の CompletionItemKind
fn completion_kind(k: symbols::SymKind) -> u32 {
    use symbols::SymKind as K;
    match k {
        K::Method => 2,
        K::Function => 3,
        K::Field => 5,
        K::Module => 9,
        K::Struct | K::Union | K::TypeAlias => 22,
        K::Enum => 13,
        K::Variant => 20,
        K::Trait => 8,
        K::Const | K::Static => 21,
        K::Macro => 15,
        _ => 1,
    }
}

/// p がクレートの src の下か (build.rs・tests・examples・benches は外)
fn in_src(root: Option<&Path>, p: &Path) -> bool {
    let rel = match root.and_then(|r| p.strip_prefix(r).ok()) {
        Some(r) => r,
        None => return true,
    };
    rel.components().any(|c| c.as_os_str() == "src")
}

/// std / core / alloc の定義の索引 (rustc --print sysroot の rust-src。なければ空)
fn index_std() -> Vec<(String, FileIndex)> {
    let sysroot = std::env::var("CRUSTC_LSP_SYSROOT").ok().or_else(|| {
        let o = std::process::Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
    });
    let Some(sysroot) = sysroot else { return vec![] };
    let lib = Path::new(&sysroot).join("lib/rustlib/src/rust/library");
    let mut out = vec![];
    for krate in ["core", "alloc", "std"] {
        let dir = lib.join(krate).join("src");
        let mut paths = vec![];
        rs_files(&dir, &mut paths, 0);
        for p in paths {
            let rel = p.strip_prefix(&lib).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
            // (内部の実装 (sys)・テスト・ほかの OS 向けのものは入れない: 定義の 4 割)
            if skip_lib(&rel) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            out.push((path_uri(&p), FileIndex::library(text, navigate::module_of(&rel))));
        }
    }
    out
}

/// std の索引に入れないファイル (library からの相対パス)
fn skip_lib(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.iter().any(|p| matches!(*p, "tests" | "benches" | "sys" | "sys_common")) {
        return true;
    }
    // std/src/os/<os>/: 使う OS (unix / linux) と共通のものだけ
    if parts.len() > 4 && parts[0] == "std" && parts[2] == "os" {
        let keep = if cfg!(target_os = "linux") { ["unix", "linux", "fd", "raw", "net"] } else { ["unix", "fd", "raw", "net", "windows"] };
        return !keep.contains(&parts[3]);
    }
    false
}

/// (行, UTF-16 の桁) → byte 位置
fn offset(text: &str, line: u32, ch: u32) -> Option<usize> {
    let mut at = 0usize;
    for _ in 0..line {
        at += text[at..].find('\n')? + 1;
    }
    let mut col = 0u32;
    for (i, c) in text[at..].char_indices() {
        if col >= ch || c == '\n' {
            return Some(at + i);
        }
        col += c.len_utf16() as u32;
    }
    Some(text.len())
}

/// f の a..b の両端の (行, UTF-16 の桁)。ライブラリのファイルは本文を持たないので、索引が先に計算したもの
fn ends(f: &FileIndex, li: &mut Option<LineIndex>, a: usize, b: usize) -> ((u32, u32), (u32, u32)) {
    if f.slim {
        return (f.lib_pos(a).unwrap_or((0, 0)), f.lib_pos(b).unwrap_or((0, 0)));
    }
    let li = li.get_or_insert_with(|| LineIndex::new(&f.text));
    (li.pos(&f.text, a), li.pos(&f.text, b))
}

fn range(text: &str, li: &LineIndex, a: usize, b: usize) -> Value {
    let (l0, c0) = li.pos(text, a);
    let (l1, c1) = li.pos(text, b);
    json!({ "start": {"line": l0, "character": c0}, "end": {"line": l1, "character": c1} })
}

/// `file:///a/b%20c.rs` → `/a/b c.rs`
fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

fn path_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for &c in p.to_string_lossy().as_bytes() {
        if c.is_ascii_alphanumeric() || b"/-_.~".contains(&c) {
            s.push(c as char);
        } else {
            s.push_str(&format!("%{c:02X}"));
        }
    }
    s
}

/// dir の下の .rs ファイル (隠しディレクトリ・target・out・node_modules は見ない。合わせて 20000 個まで)
fn rs_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    if depth > 16 || out.len() >= 20000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut es: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    es.sort();
    for p in es {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "out" && name != "node_modules" {
                rs_files(&p, out, depth + 1);
            }
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

fn publish(uri: &str, version: Value, diags: Vec<Value>) -> String {
    let mut params = json!({ "uri": uri, "diagnostics": diags });
    if !version.is_null() {
        params["version"] = version;
    }
    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params})
        .to_string()
}

/// `Content-Length: N\r\n\r\n<body>` を 1 つ読む。EOF なら None
pub fn read_msg<R: BufRead>(r: &mut R) -> io::Result<Option<String>> {
    let mut len = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let len =
        len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write_msg<W: Write>(w: &mut W, body: &str) -> io::Result<()> {
    write!(w, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    w.flush()
}

pub fn run<R: BufRead, W: Write>(mut r: R, mut w: W) -> io::Result<()> {
    let mut srv = Server::new();
    while let Some(msg) = read_msg(&mut r)? {
        for out in srv.handle(&msg) {
            write_msg(&mut w, &out)?;
        }
        if srv.exit {
            break;
        }
    }
    Ok(())
}

/// Cargo のワークスペースのファイル: (パス, モジュールのパス, ターゲットのクレートの番号, 本体のクレートか)。
/// ルートの Cargo.toml の members (と [package]) のパッケージの、ターゲットの根 (src/lib.rs・src/main.rs・src/bin・
/// tests・benches・examples (autoexamples = false でなければ)・build.rs・[[bin]] などの path) から `mod x;` をたどる。
/// Cargo.toml がなければ None
/// Cargo.toml の `[features]` の default から有効になる feature (default が含むものもたどる。`dep:` は除く)
fn default_features(toml: &str) -> Vec<String> {
    let mut table: HashMap<String, Vec<String>> = HashMap::new();
    let mut in_feat = false;
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_feat = l == "[features]";
            continue;
        }
        if !in_feat || l.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let vs: Vec<String> = v.trim().trim_start_matches('[').split(']').next().unwrap_or("").split(',').map(|x| x.trim().trim_matches('"').to_string()).filter(|x| !x.is_empty()).collect();
            table.insert(k.trim().to_string(), vs);
        }
    }
    let mut out: Vec<String> = vec![];
    let mut stack: Vec<String> = table.get("default").cloned().unwrap_or_default();
    while let Some(f) = stack.pop() {
        if f.starts_with("dep:") || f.contains('/') || out.contains(&f) {
            continue;
        }
        if let Some(more) = table.get(&f) {
            stack.extend(more.iter().cloned());
        }
        out.push(f);
    }
    out
}

#[allow(clippy::type_complexity)]
fn cargo_files(root: &Path) -> Option<(Vec<(PathBuf, Vec<String>, u32, bool)>, HashMap<u32, Vec<String>>, Vec<(String, PathBuf)>)> {
    let top = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut pkgs: Vec<PathBuf> = vec![];
    if top.contains("[package]") {
        pkgs.push(root.to_path_buf());
    }
    if let Some(i) = top.find("members") {
        if let (Some(a), Some(b)) = (top[i..].find('['), top[i..].find(']')) {
            for m in top[i + a + 1..i + b].split(',') {
                let m = m.trim().trim_matches('"').trim();
                if m.is_empty() {
                    continue;
                }
                if let Some(base) = m.strip_suffix("/*") {
                    if let Ok(rd) = std::fs::read_dir(root.join(base)) {
                        let mut ds: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("Cargo.toml").exists()).collect();
                        ds.sort();
                        pkgs.extend(ds);
                    }
                } else {
                    pkgs.push(root.join(m));
                }
            }
        }
    }
    let mut out: Vec<(PathBuf, Vec<String>, u32, bool)> = vec![];
    let mut feats: HashMap<u32, Vec<String>> = HashMap::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut krate = 0u32;
    let mut names: Vec<(String, PathBuf)> = vec![];
    for pkg in pkgs {
        let toml = std::fs::read_to_string(pkg.join("Cargo.toml")).unwrap_or_default();
        if let Some(r) = ["src/lib.rs", "src/main.rs"].iter().map(|f| pkg.join(f)).find(|p| p.exists()) {
            let n = pkg_name(&toml).replace('-', "_");
            if !n.is_empty() && !names.iter().any(|x| x.0 == n) {
                names.push((n, r));
            }
        }
        // (ターゲットの根: (ファイル, 本体か))
        let mut roots: Vec<(PathBuf, bool)> = vec![];
        for f in ["src/lib.rs", "src/main.rs"] {
            roots.push((pkg.join(f), true));
        }
        let ls = |d: &str| -> Vec<PathBuf> {
            let mut v: Vec<PathBuf> = std::fs::read_dir(pkg.join(d)).map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e == "rs")).collect()).unwrap_or_default();
            v.sort();
            v
        };
        for f in ls("src/bin") {
            roots.push((f, true));
        }
        for d in ["tests", "benches"] {
            for f in ls(d) {
                roots.push((f, false));
            }
        }
        if !toml.replace(' ', "").contains("autoexamples=false") {
            for f in ls("examples") {
                roots.push((f, false));
            }
        }
        roots.push((pkg.join("build.rs"), false));
        for l in toml.lines() {
            let l = l.trim();
            if let Some(v) = l.strip_prefix("path").map(|v| v.trim_start()).and_then(|v| v.strip_prefix('=')) {
                let v = v.trim().trim_matches('"');
                if v.ends_with(".rs") {
                    roots.push((pkg.join(v), false));
                }
            }
        }
        for (r, main) in roots {
            if !r.exists() || seen.contains(&r) {
                continue;
            }
            krate += 1;
            feats.insert(krate, default_features(&toml));
            let mut stack: Vec<(PathBuf, Vec<String>, bool)> = vec![(r, vec![], true)];
            while let Some((f, m, is_root)) = stack.pop() {
                if !seen.insert(f.clone()) {
                    continue;
                }
                let text = std::fs::read_to_string(&f).unwrap_or_default();
                // (子のファイルの場所: 根と mod.rs は同じディレクトリ、a.rs は a/ の下)
                let dir = if is_root || f.file_name().is_some_and(|n| n == "mod.rs") {
                    f.parent().map(|p| p.to_path_buf()).unwrap_or_default()
                } else {
                    f.with_extension("")
                };
                for (inline, name, path) in mod_decls(&text) {
                    let mut cm = m.clone();
                    cm.extend(inline.iter().cloned());
                    cm.push(name.clone());
                    let mut d = dir.clone();
                    for x in &inline {
                        d = d.join(x);
                    }
                    let cand = match path {
                        // (`#[path]` はファイルのあるディレクトリから。inline の mod の中ならその下から)
                        Some(p) if inline.is_empty() => vec![f.parent().map(|x| x.join(&p)).unwrap_or_default()],
                        Some(p) => vec![d.join(p)],
                        None => vec![d.join(format!("{name}.rs")), d.join(&name).join("mod.rs")],
                    };
                    if let Some(c) = cand.into_iter().find(|c| c.exists()) {
                        stack.push((c, cm, false));
                    }
                }
                // (`include!("x.rs")` / `include!(concat!(env!("OUT_DIR"), "/x.rs"))`: 同じモジュールの続き。OUT_DIR は
                // target の下のビルドスクリプトの出力 (いちばん新しいもの))
                for (out_dir, rel) in includes(&text) {
                    let c = if out_dir { out_dir_file(root, &pkg_name(&toml), &rel) } else { f.parent().map(|p| p.join(&rel)).filter(|c| c.exists()) };
                    if let Some(c) = c {
                        stack.push((c, m.clone(), false));
                    }
                }
                out.push((f, m, krate, main));
            }
        }
    }
    Some((out, feats, names))
}

/// ファイルの中の `include!("x")` と `include!(concat!(env!("OUT_DIR"), "/x"))`: (OUT_DIR の下か, パス)
fn includes(text: &str) -> Vec<(bool, String)> {
    let mut out = vec![];
    for (i, _) in text.match_indices("include!(") {
        let r = text[i + 9..].trim_start();
        let lit = |s: &str| -> Option<String> { s.strip_prefix('"').and_then(|s| s.split('"').next()).map(|x| x.to_string()) };
        if let Some(r) = r.strip_prefix("concat!(") {
            let r = r.trim_start();
            if let Some(r) = r.strip_prefix("env!(\"OUT_DIR\")") {
                if let Some(p) = r.trim_start().strip_prefix(',').and_then(|r| lit(r.trim_start())) {
                    out.push((true, p.trim_start_matches('/').to_string()));
                }
            }
        } else if let Some(p) = lit(r) {
            if p.ends_with(".rs") {
                out.push((false, p));
            }
        }
    }
    out
}

/// Cargo.toml の [package] の name
fn pkg_name(toml: &str) -> String {
    let mut in_pkg = false;
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            in_pkg = l == "[package]";
        } else if in_pkg {
            if let Some(v) = l.strip_prefix("name").map(|v| v.trim_start()).and_then(|v| v.strip_prefix('=')) {
                return v.trim().trim_matches('"').to_string();
            }
        }
    }
    String::new()
}

/// パッケージ pkg のビルドスクリプトの OUT_DIR の中のファイル rel (target/<profile>/build/<pkg>/<hash>/out/ か
/// target/<profile>/build/<pkg>-<hash>/out/ の、いちばん新しいもの)
fn out_dir_file(root: &Path, pkg: &str, rel: &str) -> Option<PathBuf> {
    if pkg.is_empty() {
        return None;
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target"));
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut consider = |p: PathBuf| {
        if let Ok(t) = std::fs::metadata(&p).and_then(|m| m.modified()) {
            if best.as_ref().is_none_or(|b| t > b.0) {
                best = Some((t, p));
            }
        }
    };
    for prof in std::fs::read_dir(&target).into_iter().flatten().flatten() {
        let build = prof.path().join("build");
        for e in std::fs::read_dir(&build).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n == pkg {
                for h in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                    consider(h.path().join("out").join(rel));
                }
            } else if n.strip_prefix(pkg).is_some_and(|r| r.starts_with('-') && r[1..].chars().all(|c| c.is_ascii_hexdigit())) {
                consider(e.path().join("out").join(rel));
            }
        }
    }
    best.map(|b| b.1)
}

/// ファイルの中の `mod x;` (囲む `mod a { .. }` の名前の列, 名前, `#[path = ".."]`)
fn mod_decls(text: &str) -> Vec<(Vec<String>, String, Option<String>)> {
    let s = text.as_bytes();
    let t = crustc_core::lexer::tokenize(s);
    let tk = |k: usize| -> &str { &text[t[k].1..t[k].2] };
    let sym = |k: usize, c: u8| k < t.len() && t[k].0 == 1 && s[t[k].1] == c;
    let mut out = vec![];
    // (inline の mod: (名前, 開いた深さ))
    let mut inl: Vec<(String, i32)> = vec![];
    let mut depth = 0i32;
    let mut k = 0;
    while k < t.len() {
        if sym(k, b'{') {
            depth += 1;
        } else if sym(k, b'}') {
            depth -= 1;
            if inl.last().is_some_and(|x| x.1 > depth) {
                inl.pop();
            }
        } else if t[k].0 == 0 && tk(k) == "mod" && k + 2 < t.len() && t[k + 1].0 == 0 {
            let name = tk(k + 1).to_string();
            if sym(k + 2, b';') {
                // (`#[path = "x.rs"]`: mod の前の属性)
                let mut path = None;
                let mut j = k;
                while j >= 1 && (t[j - 1].0 == 0 && matches!(tk(j - 1), "pub" | "crate" | "in" | "super") || sym(j - 1, b')') || sym(j - 1, b'(')) {
                    j -= 1;
                }
                if j >= 6 && sym(j - 1, b']') && t[j - 2].0 == 2 && sym(j - 3, b'=') && t[j - 4].0 == 0 && tk(j - 4) == "path" {
                    path = Some(tk(j - 2).trim_matches('"').to_string());
                }
                out.push((inl.iter().map(|x| x.0.clone()).collect(), name, path));
            } else if sym(k + 2, b'{') {
                inl.push((name, depth + 1));
            }
        }
        k += 1;
    }
    out
}
