"""syn・Cranelift を使わないコンパイラ全体 (driver2 + acg + minclif と、それが使うモジュール) を 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_compiler.py > out/selfhost/compiler_main.rs
main は `compiler <in.rs> -o <out> [-O]` で driver2::build を呼ぶ (crustc の CRUSTC_DRIVER2=1 と同じ)。
これを crustc でコンパイルしたものが、crustc 自身でコンパイルした crustc (の syn を使わない経路) になる。
proc_macro2 は tests/selfhost/mini_pm2.rs で代える (macros.rs が使う)。"""
import glob, os, re

def body(p):
    return open(p).read()

items = max(glob.glob("target/release/build/crustc-*/out/prelude_items.rs"), key=os.path.getmtime)
prelude = (body("src/prelude.rs")
    .replace('include_str!("prelude_lib.rs.in")', 'include_str!("../../src/prelude_lib.rs.in")')
    .replace('include!(concat!(env!("OUT_DIR"), "/prelude_items.rs"));', open(items).read()))

# acg/mod.rs の `pub mod clif;` / `mod p1_expr;` を中身で置き換える
acg = body("src/acg/mod.rs")
def inline(m):
    vis, name = m.group(1) or "", m.group(2)
    return f"{vis}mod {name} {{\n" + body(f"src/acg/{name}.rs") + "\n}\n"
acg = re.sub(r"^(pub )?mod (\w+);\n", inline, acg, flags=re.M)
# (`#![allow(..)]` はモジュールの先頭にしか書けない: 中に入れると内側の属性になるので外す)
acg = re.sub(r"^#!\[allow\([^)]*\)\]\n", "", acg, flags=re.M)

parts = [("key", body("src/key.rs")), ("fxhash", body("src/fxhash.rs")), ("lexer", body("src/lexer.rs")),
         ("prelude", prelude), ("types", body("src/types.rs")), ("parse", body("src/parse.rs")),
         ("ast_types", body("src/ast_types.rs")), ("ast_closure", body("src/ast_closure.rs")),
         ("ast_summary", body("src/ast_summary.rs")), ("program", body("src/program.rs")),
         ("ast_consteval", body("src/ast_consteval.rs")), ("solver", body("src/solver.rs")), ("ctors", body("src/ctors.rs")),
         ("ast_infer", body("src/ast_infer.rs")), ("minclif", body("src/minclif.rs")), ("link", body("src/link.rs")),
         ("macros", body("src/macros.rs") + "\nuse crate::proc_macro2;\n"), ("acg", acg), ("driver2", body("src/driver2.rs"))]
out = ["// 自動生成: tests/selfhost/build_compiler.py (crustc の src/*.rs をそのまま含む)",
       "mod cache {\n    pub use crate::key::*;\n}\n", body("tests/selfhost/mini_pm2.rs")]
for name, src in parts:
    out.append(f"mod {name} {{\n" + src + "\n}\n")
out.append(r"""
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut opt = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                output = args.get(i).cloned();
            }
            "-O" => opt = true,
            a => input = Some(a.to_string()),
        }
        i += 1;
    }
    let Some(input) = input else {
        eprintln!("usage: compiler <in.rs> -o <out> [-O]");
        std::process::exit(2);
    };
    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {input}: {e}");
            std::process::exit(1);
        }
    };
    let out = output.unwrap_or_else(|| input.trim_end_matches(".rs").to_string());
    if let Err(e) = driver2::build(&src, std::path::Path::new(&out), opt, None) {
        eprintln!("error: {input}: {e}");
        std::process::exit(1);
    }
}
""")
print("\n".join(out))
