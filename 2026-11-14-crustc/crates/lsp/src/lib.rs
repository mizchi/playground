//! 最小の LSP サーバ (stdio)。対応: initialize / didOpen / didChange (全文同期) / didClose /
//! shutdown / exit / textDocument/documentSymbol / workspace/symbol / textDocument/definition / textDocument/references。
//! 開く・変更するたびに [`Session2::check`] を走らせて publishDiagnostics を返す。
//!
//! ドキュメントごとに Session2 を 1 つ持ち、キャッシュはメモリ上だけに置く。
//! 変わっていない関数は「検査済み」の印で飛ばすので、応答時間は変更量に比例する。
//! シンボルは crustc_syntax::symbols で字句の列から読む (構文が壊れていても読める所まで)。workspace/symbol・定義・参照は
//! 開いているドキュメントと、ワークスペースの根の下の .rs ファイル (更新時刻と大きさで覚える) の索引
//! (crustc_syntax::navigate::FileIndex) から引く。

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crustc_core::driver2::{Options2, Session2};
use crustc_core::navigate::{self, FileIndex, Loc};
use crustc_core::symbols::{self, LineIndex, Symbol};

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
            exit: false,
            shutdown: false,
        }
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
                    "capabilities": { "textDocumentSync": 1, "documentSymbolProvider": true, "workspaceSymbolProvider": true, "definitionProvider": true, "referencesProvider": true },
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
                        "kind": s.kind.lsp(),
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
                Some((_, Some(v))) => v,
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
                d.idx = Some(ix);
            }
            open.push(p);
        }
        if root.is_some() {
            let paths = layout.clone();
            let mut changed = false;
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
        let mut out: Vec<(String, &FileIndex)> = vec![];
        for (uri, d) in &self.docs {
            out.push((uri.clone(), d.idx.as_ref().unwrap()));
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

    /// workspace/symbol: 名前が問い合わせに合うシンボル (SymbolInformation)。合い方の良い順に 256 個まで
    fn workspace_symbols(&mut self, q: &str) -> Value {
        // (std は入れない: rust-analyzer の既定と同じくワークスペースだけ)
        let ws = self.workspace(false);
        // (候補: (良さ, 名前, 種類, uri, 範囲, 囲むもの))
        let mut hits: Vec<(u32, String, u32, String, [u32; 4], String)> = vec![];
        for (uri, f) in &ws {
            let mut li: Option<LineIndex> = None;
            for d in &f.defs {
                if let Some(sc) = symbols::match_score(q, &d.name) {
                    let ((l0, c0), (l1, c1)) = ends(f, &mut li, d.start, d.end);
                    hits.push((sc, d.name.clone(), d.kind.lsp(), uri.clone(), [l0, c0, l1, c1], d.container.clone()));
                }
            }
        }
        hits.sort_by(|a, b| (a.0, a.1.len(), &a.1, &a.3, a.4).cmp(&(b.0, b.1.len(), &b.1, &b.3, b.4)));
        hits.truncate(256);
        json!(hits
            .into_iter()
            .map(|(_, name, kind, uri, r, c)| {
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

    /// textDocument/definition (refs = false) と textDocument/references (refs = true)
    fn navigate(&mut self, uri: &str, line: u32, ch: u32, refs: bool, decl: bool) -> Value {
        let ws = self.workspace(true);
        // (開いているドキュメントか、ワークスペースのファイル。uri の書き方の違いはパスで比べる)
        let want = uri_path(uri);
        let Some(fi) = ws.iter().position(|(u, _)| u == uri || (want.is_some() && uri_path(u) == want)) else { return Value::Null };
        let Some(off) = offset(&ws[fi].1.text, line, ch) else { return Value::Null };
        let idx: Vec<&FileIndex> = ws.iter().map(|x| x.1).collect();
        let locs: Vec<Loc> = if refs { navigate::references(&idx, fi, off, decl) } else { navigate::definition(&idx, fi, off) };
        let mut lis: HashMap<usize, Option<LineIndex>> = HashMap::new();
        let out: Vec<Value> = locs
            .iter()
            .map(|l| {
                let ((l0, c0), (l1, c1)) = ends(idx[l.file], lis.entry(l.file).or_default(), l.start, l.end);
                json!({ "uri": ws[l.file].0, "range": { "start": {"line": l0, "character": c0}, "end": {"line": l1, "character": c1} } })
            })
            .collect();
        if refs || out.len() != 1 { json!(out) } else { out.into_iter().next().unwrap() }
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
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let rel = p.strip_prefix(&lib).map(|r| r.to_string_lossy().into_owned()).unwrap_or_default();
            out.push((path_uri(&p), FileIndex::library(text, navigate::module_of(&rel))));
        }
    }
    out
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
    if f.lib {
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
fn cargo_files(root: &Path) -> Option<Vec<(PathBuf, Vec<String>, u32, bool)>> {
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
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut krate = 0u32;
    for pkg in pkgs {
        let toml = std::fs::read_to_string(pkg.join("Cargo.toml")).unwrap_or_default();
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
                        Some(p) => vec![d.join(p)],
                        None => vec![d.join(format!("{name}.rs")), d.join(&name).join("mod.rs")],
                    };
                    if let Some(c) = cand.into_iter().find(|c| c.exists()) {
                        stack.push((c, cm, false));
                    }
                }
                out.push((f, m, krate, main));
            }
        }
    }
    Some(out)
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
