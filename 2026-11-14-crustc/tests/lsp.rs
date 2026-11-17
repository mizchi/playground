//! `crustc lsp` を実際に起動し、stdio 越しに診断が返ることを確かめる。

use std::io::{BufReader, Write};
use std::process::{Command, Stdio};

use crustc::lsp::{read_msg, write_msg};
use serde_json::{Value, json};

#[test]
fn diagnostics_roundtrip() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_crustc"))
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
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("mismatched types")
    );

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

/// documentSymbol (開いたドキュメントの木) と workspace/symbol (開いたドキュメントとワークスペースの .rs から名前で探す)
#[test]
fn symbols() {
    use crustc::lsp::Server;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lsp_symbols");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("src/geom.rs"), "pub struct Point { pub x: f64, pub y: f64 }\nimpl Point {\n    pub fn norm(&self) -> f64 { 0.0 }\n}\n").unwrap();
    // (target の下は見ない)
    std::fs::write(root.join("target/junk.rs"), "fn point_junk() {}\n").unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"]["capabilities"]["workspaceSymbolProvider"], true);
    assert_eq!(r["result"]["capabilities"]["documentSymbolProvider"], true);

    let uri = format!("{uri_root}/src/main.rs");
    let text = "mod util {\n    pub fn point_dist() -> u32 { 1 }\n}\nenum Shape { Circle(f64), Rect { w: f64 } }\nfn main() {}\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());

    // documentSymbol: 木と位置 (行・桁は 0 始まり)
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/documentSymbol", "params": {"textDocument": {"uri": uri}}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    let syms = r["result"].as_array().unwrap();
    let names: Vec<&str> = syms.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["util", "Shape", "main"]);
    assert_eq!(syms[0]["kind"], 2);
    assert_eq!(syms[0]["children"][0]["name"], "point_dist");
    assert_eq!(syms[0]["children"][0]["selectionRange"]["start"], json!({"line": 1, "character": 11}));
    let vars: Vec<&str> = syms[1]["children"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(vars, ["Circle", "Rect"]);
    assert_eq!(syms[1]["children"][1]["children"][0]["name"], "w");

    // workspace/symbol: 開いたドキュメントと、開いていない src/geom.rs から。前方一致が先
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "workspace/symbol", "params": {"query": "point"}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    let hits: Vec<(String, String)> = r["result"].as_array().unwrap().iter()
        .map(|s| (s["name"].as_str().unwrap().to_string(), s["location"]["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_string()))
        .collect();
    assert_eq!(hits[0], ("Point".to_string(), "geom.rs".to_string()), "{hits:?}");
    assert!(hits.contains(&("point_dist".to_string(), "main.rs".to_string())), "{hits:?}");
    assert!(hits.contains(&("impl Point".to_string(), "geom.rs".to_string())), "{hits:?}");
    assert!(!hits.iter().any(|h| h.0 == "point_junk"), "{hits:?}");
    let pd = r["result"].as_array().unwrap().iter().find(|s| s["name"] == "point_dist").unwrap();
    assert_eq!(pd["containerName"], "util");
    assert_eq!(pd["kind"], 12);

    // 文字の順に含むもの (`nrm` → norm) と、ファイルを書き換えたら読み直す
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "workspace/symbol", "params": {"query": "nrm"}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"][0]["name"], "norm");
    assert_eq!(r["result"][0]["containerName"], "impl Point");
    std::fs::write(root.join("src/geom.rs"), "pub struct Point2 {}\n// changed file\n").unwrap();
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "workspace/symbol", "params": {"query": "nrm"}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"].as_array().unwrap().len(), 0, "{r}");

    // 編集した本文のシンボル
    let text2 = "fn renamed_fn() {}\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
        "params": {"textDocument": {"uri": uri, "version": 2}, "contentChanges": [{"text": text2}]}}).to_string());
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 6, "method": "workspace/symbol", "params": {"query": "renamed"}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"][0]["name"], "renamed_fn");
}

/// definition / references: ローカル・メソッド (受け手の型から)・`use` したアイテム・ファイルをまたぐ参照
#[test]
fn definition_and_references() {
    nav(usize::MAX, "lsp_nav");
}

/// 同じことを、ワークスペースのファイルを全部 slim にして (本文を捨て、参照のときに読み直す)
#[test]
fn definition_and_references_slim() {
    nav(0, "lsp_nav_slim");
}

fn nav(budget: usize, dir: &str) {
    use crustc::lsp::Server;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(dir);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/geom.rs"), "pub struct Point { pub x: f64 }\nimpl Point {\n    pub fn new() -> Point { Point { x: 0.0 } }\n    pub fn norm(&self) -> f64 { self.x }\n}\npub fn norm() {}\n").unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    srv.set_mem_budget(budget);
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"]["capabilities"]["definitionProvider"], true);
    assert_eq!(r["result"]["capabilities"]["referencesProvider"], true);
    let uri = format!("{uri_root}/src/main.rs");
    let text = "mod geom;\nuse geom::Point;\nfn main() {\n    let p = Point::new();\n    let n = p.norm();\n    helper(n);\n}\nfn helper(x: f64) -> f64 { x }\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());
    let mut id = 10;
    let mut ask = |method: &str, line: u32, ch: u32| -> Vec<(String, u64, u64)> {
        id += 1;
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"textDocument": {"uri": uri},
            "position": {"line": line, "character": ch}, "context": {"includeDeclaration": true}}}).to_string());
        let r: Value = serde_json::from_str(&r[0]).unwrap();
        let v = match &r["result"] {
            Value::Array(a) => a.clone(),
            Value::Null => vec![],
            o => vec![o.clone()],
        };
        let mut out: Vec<(String, u64, u64)> = v.iter()
            .map(|l| (l["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_string(), l["range"]["start"]["line"].as_u64().unwrap(), l["range"]["start"]["character"].as_u64().unwrap()))
            .collect();
        out.sort();
        out
    };
    let at = |f: &str, l: u64, c: u64| (f.to_string(), l, c);
    // `Point::new()` の Point → geom.rs の struct、new → impl のメソッド
    assert_eq!(ask("textDocument/definition", 3, 12), [at("geom.rs", 0, 11)]);
    assert_eq!(ask("textDocument/definition", 3, 19), [at("geom.rs", 2, 11)]);
    // `p.norm()`: p の型 (Point) のメソッド。同じ名前の関数 geom::norm ではない
    assert_eq!(ask("textDocument/definition", 4, 14), [at("geom.rs", 3, 11)]);
    // ローカル
    assert_eq!(ask("textDocument/definition", 5, 11), [at("main.rs", 4, 8)]);
    assert_eq!(ask("textDocument/definition", 7, 28), [at("main.rs", 7, 10)]);
    // 参照: helper は宣言と呼び出し、Point はファイルをまたいで (use・パス・impl・戻り値の型・構造体の式)
    assert_eq!(ask("textDocument/references", 7, 4), [at("main.rs", 5, 4), at("main.rs", 7, 3)]);
    let refs = ask("textDocument/references", 0, 12);
    assert!(refs.is_empty(), "{refs:?}"); // (`mod geom` の geom はモジュール: 参照は返さない)
    let refs = ask("textDocument/references", 1, 11);
    assert_eq!(refs, [at("geom.rs", 0, 11), at("geom.rs", 1, 5), at("geom.rs", 2, 20), at("geom.rs", 2, 28), at("main.rs", 1, 10), at("main.rs", 3, 12)]);
}

/// hover と completion: シグネチャ・doc コメント・ローカルの型、`x.|` (受け手の型のメンバ)、`Q::|`、名前の途中
#[test]
fn hover_and_completion() {
    use crustc::lsp::Server;
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lsp_hover");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/geom.rs"), "/// 平面の点\npub struct Point { pub x: f64, pub y: f64 }\nimpl Point {\n    /// 原点\n    pub fn origin() -> Point { Point { x: 0.0, y: 0.0 } }\n    pub fn norm(&self) -> f64 { self.x }\n    pub fn shifted(&self) -> Option<Point> { None }\n}\n").unwrap();
    let uri_root = format!("file://{}", root.display());
    let mut srv = Server::new();
    let r = srv.handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": uri_root}}).to_string());
    let r: Value = serde_json::from_str(&r[0]).unwrap();
    assert_eq!(r["result"]["capabilities"]["hoverProvider"], true);
    assert!(r["result"]["capabilities"]["completionProvider"].is_object());
    let uri = format!("{uri_root}/src/main.rs");
    let text = "mod geom;\nuse geom::Point;\nfn main() {\n    let p = Point::origin();\n    let q = p.shifted().unwrap();\n    let n = q.norm();\n    let v: Vec<Point> = vec![];\n    v.\n    Point::\n    no\n}\n";
    srv.handle(&json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}}}).to_string());
    let mut id = 10;
    let mut ask = |method: &str, line: u32, ch: u32| -> Value {
        id += 1;
        let r = srv.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"textDocument": {"uri": uri},
            "position": {"line": line, "character": ch}}}).to_string());
        let r: Value = serde_json::from_str(&r[0]).unwrap();
        r["result"].clone()
    };
    let hover = |v: Value| v["contents"]["value"].as_str().unwrap_or("").to_string();
    let labels = |v: Value| -> Vec<String> { v["items"].as_array().unwrap().iter().map(|x| x["label"].as_str().unwrap().to_string()).collect() };
    // 定義のシグネチャと doc コメント
    let h = hover(ask("textDocument/hover", 3, 20));
    assert!(h.contains("pub fn origin() -> Point") && h.contains("原点") && h.contains("impl Point"), "{h}");
    let h = hover(ask("textDocument/hover", 1, 11));
    assert!(h.contains("pub struct Point") && h.contains("平面の点"), "{h}");
    // ローカルの型: Option<Point>::unwrap → Point
    assert!(hover(ask("textDocument/hover", 4, 8)).contains("q: Point"));
    assert!(hover(ask("textDocument/hover", 5, 8)).contains("n: f64"));
    // q.norm() は Point::norm
    assert!(hover(ask("textDocument/hover", 5, 15)).contains("pub fn norm(&self) -> f64"));
    // `p.|`: Point のメンバ、`v.|`: Vec (とスライス) のメソッド
    let c = labels(ask("textDocument/completion", 4, 14));
    assert!(c.contains(&"shifted".to_string()) && c.contains(&"x".to_string()) && !c.contains(&"push".to_string()), "{c:?}");
    let c = labels(ask("textDocument/completion", 7, 6));
    if std::env::var_os("CRUSTC_LSP_NO_STD").is_none() && c.iter().any(|x| x == "len") {
        assert!(c.contains(&"push".to_string()) && c.contains(&"iter".to_string()), "{c:?}");
    }
    // `Point::|`: 関連関数とフィールドでないもの
    let c = labels(ask("textDocument/completion", 8, 11));
    assert!(c.contains(&"origin".to_string()), "{c:?}");
    // 名前の途中 `no|`: ローカルの n ではなく、n で始まる / 含むもの (norm は Point のメンバなので出ない)
    let c = labels(ask("textDocument/completion", 9, 6));
    assert!(!c.is_empty() && c.iter().all(|x| x.to_lowercase().contains('n')), "{c:?}");
}
