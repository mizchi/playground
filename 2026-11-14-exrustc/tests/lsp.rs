//! `exrustc lsp` を実際に起動し、stdio 越しに診断が返ることを確かめる。

use std::io::{BufReader, Write};
use std::process::{Command, Stdio};

use exrustc::lsp::{read_msg, write_msg};
use serde_json::{Value, json};

#[test]
fn diagnostics_roundtrip() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_exrustc"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut send = |v: Value| write_msg(&mut stdin, &v.to_string()).unwrap();
    let uri = "file:///t.rs";

    send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
    let r: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["capabilities"]["textDocumentSync"], 1);
    send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));

    // 型エラーのある版を開く
    let bad = "fn f() -> u8 {\n    1i64\n}\nfn main() {}\n";
    send(json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": bad}}}));
    let d: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(d["method"], "textDocument/publishDiagnostics");
    let diags = d["params"]["diagnostics"].as_array().unwrap();
    assert_eq!(diags.len(), 1, "{d}");
    assert_eq!(diags[0]["range"]["start"]["line"], 1);
    assert!(diags[0]["message"].as_str().unwrap().contains("mismatched types"));

    // 直すと診断が消える
    let good = "fn f() -> u8 {\n    1\n}\nfn main() {}\n";
    send(json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
        "params": {"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": good}]}}));
    let d: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(d["params"]["version"], 2);
    assert_eq!(d["params"]["diagnostics"].as_array().unwrap().len(), 0);

    send(json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}));
    let r: Value = serde_json::from_str(&read_msg(&mut stdout).unwrap().unwrap()).unwrap();
    assert_eq!(r["id"], 2);
    send(json!({"jsonrpc": "2.0", "method": "exit"}));
    stdin.flush().ok();
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
