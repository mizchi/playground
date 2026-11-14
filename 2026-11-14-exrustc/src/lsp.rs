//! 最小の LSP サーバ (stdio)。対応: initialize / didOpen / didChange (全文同期) / didClose /
//! shutdown / exit。開く・変更するたびに [`Session::check`] を走らせて publishDiagnostics を返す。
//!
//! ドキュメントごとに Session を 1 つ持ち、キャッシュはメモリ上だけに置く。
//! 変わっていない関数は「検査済み」の印で飛ばすので、応答時間は変更量に比例する。

use std::collections::HashMap;
use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use crate::{Options, Session};

pub struct Server {
    /// uri → (セッション, 直前の本文のハッシュと診断)
    docs: HashMap<String, (Session, u64, Vec<Value>)>,
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
        Server { docs: HashMap::new(), exit: false, shutdown: false }
    }

    /// 1 メッセージを処理し、送り返すメッセージ (JSON 文字列) を返す
    pub fn handle(&mut self, msg: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(msg) else {
            return vec![json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}).to_string()];
        };
        let method = v["method"].as_str().unwrap_or("");
        let id = v.get("id").cloned();
        let p = &v["params"];
        let reply = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
        match method {
            "initialize" => vec![reply(json!({
                "capabilities": { "textDocumentSync": 1 },
                "serverInfo": { "name": "exrustc", "version": env!("CARGO_PKG_VERSION") }
            }))],
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
                let Some(text) = p["contentChanges"].as_array().and_then(|a| a.last()).and_then(|c| c["text"].as_str())
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
            _ if id.is_some() => vec![json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("method not found: {method}")}
            })
            .to_string()],
            _ => vec![], // 未対応の通知は無視
        }
    }

    fn diagnose(&mut self, uri: String, text: &str, version: Value) -> String {
        let h = {
            use std::hash::{Hash, Hasher};
            let mut h = std::hash::DefaultHasher::new();
            text.hash(&mut h);
            h.finish()
        };
        let (s, last, cached) =
            self.docs.entry(uri.clone()).or_insert_with(|| (Session::new(Options::default()), 0, vec![]));
        // 本文が同じ (保存しただけ等) なら前回の診断をそのまま返す
        if *last == h {
            return publish(&uri, version, cached.clone());
        }
        let diags: Vec<Value> = s
            .check(text)
            .into_iter()
            .map(|d| {
                let (l, c) = (d.line.saturating_sub(1), d.col.saturating_sub(1));
                json!({
                    "range": { "start": {"line": l, "character": c}, "end": {"line": l, "character": c + 1} },
                    "severity": 1,
                    "source": "exrustc",
                    "message": d.msg,
                })
            })
            .collect();
        *last = h;
        *cached = diags.clone();
        publish(&uri, version, diags)
    }
}

fn publish(uri: &str, version: Value, diags: Vec<Value>) -> String {
    let mut params = json!({ "uri": uri, "diagnostics": diags });
    if !version.is_null() {
        params["version"] = version;
    }
    json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params}).to_string()
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
    let len = len.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
    let mut buf = vec![0; len];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
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
