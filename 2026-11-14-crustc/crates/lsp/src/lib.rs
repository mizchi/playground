//! 最小の LSP サーバ (stdio)。対応: initialize / didOpen / didChange (全文同期) / didClose /
//! shutdown / exit / textDocument/documentSymbol / workspace/symbol。
//! 開く・変更するたびに [`Session2::check`] を走らせて publishDiagnostics を返す。
//!
//! ドキュメントごとに Session2 を 1 つ持ち、キャッシュはメモリ上だけに置く。
//! 変わっていない関数は「検査済み」の印で飛ばすので、応答時間は変更量に比例する。
//! シンボルは crustc_syntax::symbols で字句の列から読む (構文が壊れていても読める所まで)。workspace/symbol は
//! 開いているドキュメントと、ワークスペースの根の下の .rs ファイル (更新時刻と大きさで覚える) から探す。

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crustc_core::driver2::{Options2, Session2};
use crustc_core::symbols::{self, LineIndex, Symbol};

/// 開いているドキュメント
struct Doc {
    s: Session2,
    /// 本文と、それに対する診断・シンボル (シンボルは求められたときに作る)
    text: String,
    diags: Vec<Value>,
    syms: Option<Vec<Symbol>>,
}

/// ワークスペースのファイル 1 つ分のシンボル (平らにしたもの): (名前, 種類, 囲むもの, 範囲)
struct FileSyms {
    stamp: (u64, u64),
    syms: Vec<(String, u32, String, [u32; 4])>,
}

pub struct Server {
    docs: HashMap<String, Doc>,
    /// ワークスペースの根 (initialize の rootUri / rootPath / workspaceFolders)
    root: Option<PathBuf>,
    /// ワークスペースのファイル (開いていないもの) のシンボル
    files: HashMap<PathBuf, FileSyms>,
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
                vec![reply(json!({
                    "capabilities": { "textDocumentSync": 1, "documentSymbolProvider": true, "workspaceSymbolProvider": true },
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
            .or_insert_with(|| Doc { s: Session2::new(Options2 { jobs: 1, ..Default::default() }), text: String::new(), diags: vec![], syms: None });
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

    /// workspace/symbol: 名前が問い合わせに合うシンボル (SymbolInformation)。合い方の良い順に 256 個まで
    fn workspace_symbols(&mut self, q: &str) -> Value {
        // (候補: (良さ, 名前, 種類, uri, 範囲, 囲むもの))
        let mut hits: Vec<(u32, String, u32, String, [u32; 4], String)> = vec![];
        let mut open: Vec<PathBuf> = vec![];
        for (uri, d) in self.docs.iter_mut() {
            if let Some(p) = uri_path(uri) {
                open.push(p);
            }
            if d.syms.is_none() {
                d.syms = Some(symbols::document_symbols(&d.text));
            }
            let li = LineIndex::new(&d.text);
            for (s, c) in symbols::flatten(d.syms.as_ref().unwrap()) {
                if let Some(sc) = symbols::match_score(q, &s.name) {
                    let (l0, c0) = li.pos(&d.text, s.name_start);
                    let (l1, c1) = li.pos(&d.text, s.name_end);
                    hits.push((sc, s.name, s.kind.lsp(), uri.clone(), [l0, c0, l1, c1], c));
                }
            }
        }
        if let Some(root) = self.root.clone() {
            let mut paths = vec![];
            rs_files(&root, &mut paths, 0);
            let mut keep: HashMap<PathBuf, FileSyms> = HashMap::new();
            for p in paths {
                if open.contains(&p) {
                    continue;
                }
                let Ok(md) = std::fs::metadata(&p) else { continue };
                let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
                let stamp = (mtime, md.len());
                let fs = match self.files.remove(&p) {
                    Some(f) if f.stamp == stamp => f,
                    _ => {
                        let text = if md.len() <= 8 << 20 { std::fs::read_to_string(&p).unwrap_or_default() } else { String::new() };
                        let li = LineIndex::new(&text);
                        let syms = symbols::flatten(&symbols::document_symbols(&text))
                            .into_iter()
                            .map(|(s, c)| {
                                let (l0, c0) = li.pos(&text, s.name_start);
                                let (l1, c1) = li.pos(&text, s.name_end);
                                (s.name, s.kind.lsp(), c, [l0, c0, l1, c1])
                            })
                            .collect();
                        FileSyms { stamp, syms }
                    }
                };
                let uri = path_uri(&p);
                for (n, k, c, r) in &fs.syms {
                    if let Some(sc) = symbols::match_score(q, n) {
                        hits.push((sc, n.clone(), *k, uri.clone(), *r, c.clone()));
                    }
                }
                keep.insert(p, fs);
            }
            self.files = keep;
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
