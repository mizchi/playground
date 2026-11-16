"""syn・Cranelift を使わないコンパイラ全体 (driver2 + acg + minclif と、それが使うモジュール) を 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_compiler.py > out/selfhost/compiler_main.rs
main は `compiler <in.rs> -o <out> [-O]` で driver2::build を呼ぶ (crustc の CRUSTC_DRIVER2=1 と同じ)。
これを crustc でコンパイルしたものが、crustc 自身でコンパイルした crustc (の syn を使わない経路) になる。
proc_macro2 は tests/selfhost/mini_pm2.rs で代える (macros.rs が使う)。"""
import re, glob, os, re

# crustc はアイテム位置の macro_rules! を展開しないので enc_struct! をここで展開する
def expand_enc_struct(src):
    def ex(m):
        t, fs = m.group(1), [f.strip() for f in m.group(2).split(",") if f.strip()]
        e = "".join(f"        if !crate::enc::Enc::is_empty_val(&self.{f}) {{ mask |= bit; }} bit <<= 1;\n" for f in fs)
        w = "".join(f"        if mask & bit != 0 {{ crate::enc::Enc::enc(&self.{f}, out); }} bit <<= 1;\n" for f in fs)
        d = "".join(f"        if mask & bit != 0 {{ v.{f} = crate::enc::Enc::dec(r)?; }} bit <<= 1;\n" for f in fs)
        return (f"impl crate::enc::Enc for {t} {{\n    fn enc(&self, out: &mut Vec<u8>) {{\n        let mut mask: u64 = 0;\n        let mut bit = 1u64;\n{e}        let _ = bit;\n"
                f"        crate::enc::put_varint(out, mask);\n        let mut bit = 1u64;\n{w}        let _ = bit;\n    }}\n"
                f"    fn dec(r: &mut crate::enc::Rd) -> Option<Self> {{\n        let mask = r.varint()?;\n        let mut v = {t}::default();\n        let mut bit = 1u64;\n{d}        let _ = bit;\n        Some(v)\n    }}\n}}\n")
    return re.sub(r"crate::enc_struct!\((\w+) \{([^}]*)\}\);", ex, src)

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
         ("macros", body("src/macros.rs") + "\nuse crate::proc_macro2;\n"), ("acg", acg), ("enc", body("src/enc.rs")), ("types_enc", expand_enc_struct(body("src/types_enc.rs"))), ("cache2", body("src/cache2.rs")), ("srcmap", body("src/srcmap.rs")), ("driver2", body("src/driver2.rs"))]
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
    let mut jobs: usize = 0;
    let mut no_cache = false;
    let mut time = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                output = args.get(i).cloned();
            }
            "-O" => opt = true,
            "-j" => {
                i += 1;
                jobs = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(1);
            }
            "--no-cache" => no_cache = true,
            "--time" => time = true,
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
    // (`include_str!` / `include!` はここで展開する: main.rs と同じ)
    let dir = std::path::Path::new(&input).parent().map(|p| p.to_path_buf()).unwrap_or_else(|| std::path::PathBuf::from("."));
    let src = match macros::expand_includes(&src, dir.as_path()) {
        Ok(Some(s)) => s,
        Ok(None) => src,
        Err(e) => {
            eprintln!("error: {input}: {e}");
            std::process::exit(1);
        }
    };
    let out = output.unwrap_or_else(|| input.trim_end_matches(".rs").to_string());
    let mut o = driver2::Options2::default();
    o.opt = opt;
    o.no_cache = no_cache;
    o.time = time;
    if jobs > 0 {
        o.jobs = jobs;
    }
    let mut s = driver2::Session2::new(o);
    let cp = format!("{out}.crcache2");
    s.load_cache(std::path::Path::new(&cp));
    if let Err(e) = s.build(&src, std::path::Path::new(&out)) {
        eprintln!("error: {input}: {e}");
        std::process::exit(1);
    }
    if let Err(e) = s.save_cache(std::path::Path::new(&cp)) {
        eprintln!("warning: failed to write cache: {e}");
    }
}
""")
print("\n".join(out))
