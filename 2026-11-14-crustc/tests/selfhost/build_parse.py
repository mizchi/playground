"""crustc 自身の crates/syntax/src/parse.rs (syn を使わない Rust の構文解析) を 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_parse.py > out/selfhost/parse_main.rs
main は引数のファイルごとに構文解析し、節の数・アイテムの名前と関数のシグネチャを出す。"""

parse = open("crates/syntax/src/parse.rs").read()
print("""// 自動生成: tests/selfhost/build_parse.py (crustc の crates/syntax/src/parse.rs をそのまま含む)
mod parse {
""" + parse + """}

use parse::*;

fn item_line(it: &Item, depth: usize, out: &mut Vec<String>) {
    let pad = "  ".repeat(depth);
    match &it.kind {
        ItemKind::Fn(sig, body) => {
            let ps: Vec<String> = sig.params.iter().map(|(_, t)| ty_text(t)).collect();
            let ret = sig.ret.as_ref().map(ty_text).unwrap_or_default();
            out.push(format!("{pad}fn {}({}{}) -> {} [{} stmts]", sig.name, sig.recv.clone().map(|r| r + ", ").unwrap_or_default(), ps.join(", "), ret, body.as_ref().map(|b| b.stmts.len()).unwrap_or(0)));
        }
        ItemKind::Struct(n, g, f) => {
            let k = match f {
                Fields::Unit => 0,
                Fields::Tuple(v) => v.len(),
                Fields::Named(v) => v.len(),
            };
            out.push(format!("{pad}struct {n} <{}> {k} fields", g.params.len()));
        }
        ItemKind::Enum(n, _, vs) => {
            let names: Vec<&str> = vs.iter().map(|v| v.0.as_str()).collect();
            out.push(format!("{pad}enum {n} {}", names.join("|")));
        }
        ItemKind::Impl(_, tr, ty, items) => {
            let t = tr.as_ref().map(|(_, p)| format!("{} for ", path_text(p))).unwrap_or_default();
            out.push(format!("{pad}impl {t}{}", ty_text(ty)));
            for i in items {
                item_line(i, depth + 1, out);
            }
        }
        ItemKind::Trait(n, _, _, items) => {
            out.push(format!("{pad}trait {n}"));
            for i in items {
                item_line(i, depth + 1, out);
            }
        }
        ItemKind::Mod(n, Some(items)) => {
            out.push(format!("{pad}mod {n}"));
            for i in items {
                item_line(i, depth + 1, out);
            }
        }
        ItemKind::Use(s) => out.push(format!("{pad}use {s}")),
        ItemKind::Const(n, t, _) => out.push(format!("{pad}const {n}: {}", t.as_ref().map(ty_text).unwrap_or_default())),
        ItemKind::Static(m, n, t, _) => out.push(format!("{pad}static {}{n}: {}", if *m { "mut " } else { "" }, ty_text(t))),
        ItemKind::Type(n, _, _, t) => out.push(format!("{pad}type {n} = {}", t.as_ref().map(ty_text).unwrap_or_default())),
        ItemKind::MacroRules(n, g) => out.push(format!("{pad}macro_rules! {n} ({} bytes)", g.len())),
        _ => out.push(format!("{pad}other item at line {}", it.line)),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    for path in &args[1..] {
        let src = std::fs::read_to_string(path).unwrap();
        match parse_file(&src) {
            Ok(f) => {
                let mut c = Counts::default();
                c.file(&f);
                println!("{path}: {} items {:?}", f.items.len(), c);
                let mut out = Vec::new();
                for it in &f.items {
                    item_line(it, 1, &mut out);
                }
                for l in out {
                    println!("{l}");
                }
            }
            Err(e) => println!("{path}: error {e}"),
        }
    }
}
""")
