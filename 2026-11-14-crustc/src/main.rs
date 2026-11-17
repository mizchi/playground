//! crustc の CLI。
//!   crustc <input.rs> [-o out] [...]   ビルド (自前のフロントエンド + Cranelift。`--backend=minclif` で minclif)
//!   crustc build <workspace> [-o dir]   ワークスペース (複数のクレート) をビルド
//!   crustc check <input.rs>            型検査だけ (全診断を表示)
//!   crustc lsp                         LSP サーバ (stdio)

use std::path::PathBuf;

use crustc::driver2::{Options2, Session2};

fn usage() -> ! {
    eprintln!(
        "usage: crustc <input.rs> [-o out] [-O] [--backend=cranelift|minclif] [--linker=NAME] [--features=a,b] [--time] [-j N] [--no-cache]\n       crustc build <workspace-file> [-o out-dir] [-O] [--backend=..] [-j N] [--time] [--no-cache]\n       crustc check <input.rs>\n       crustc lsp"
    );
    std::process::exit(2)
}

fn die(msg: String) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut o = Options2::default();
    let mut input = PathBuf::new();
    let mut output = PathBuf::new();
    let mut mode = "build";
    // (バックエンド: 既定は Cranelift。CRUSTC_BACKEND=minclif / --backend=minclif で minclif)
    let mut backend = std::env::var("CRUSTC_BACKEND").unwrap_or_else(|_| "cranelift".to_string());
    match args.peek().map(String::as_str) {
        Some("check") => {
            mode = "check";
            args.next();
        }
        Some("build") => {
            mode = "workspace";
            args.next();
        }
        Some("lsp") => {
            std::process::exit(match crustc::lsp::run(std::io::stdin().lock(), std::io::stdout().lock()) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("crustc lsp: {e}");
                    1
                }
            });
        }
        _ => {}
    }
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => output = args.next().unwrap_or_else(|| usage()).into(),
            "-O" => o.opt = true,
            "--time" => o.time = true,
            "--no-cache" => o.no_cache = true,
            "-j" => o.jobs = args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()),
            s if s.starts_with("--backend=") => backend = s["--backend=".len()..].to_string(),
            s if s.starts_with("--linker=") => o.linker = Some(s["--linker=".len()..].into()),
            s if s.starts_with("--features=") => o.features.extend(s["--features=".len()..].split(',').filter(|f| !f.is_empty()).map(|f| f.to_string())),
            s if s.starts_with('-') => usage(),
            s => input = s.into(),
        }
    }
    if input.as_os_str().is_empty() {
        usage();
    }
    match backend.as_str() {
        "cranelift" => o.backend = Some(crustc::clif_backend::compile),
        "minclif" => o.backend = None,
        b => {
            eprintln!("error: unknown backend `{b}` (cranelift / minclif)");
            std::process::exit(2);
        }
    }
    if mode == "workspace" {
        let crates = crustc::driver2::read_workspace(&input).unwrap_or_else(|e| die(e));
        let out_dir = if output.as_os_str().is_empty() { input.parent().map(|d| d.join("target")).unwrap_or("target".into()) } else { output };
        // (キャッシュはクレートごとに out_dir/<名前>.crcache2)
        match Session2::new(o).build_workspace(&crates, &out_dir) {
            Ok(_) => std::process::exit(0),
            Err(e) => die(e),
        }
    }
    if mode == "check" {
        // (`mod x;` のファイルも読む。診断の行は結合したソースのもの: 1 ファイルならそのまま)
        let (src, _) = crustc::srcmap::load_crate(&input, "src/main.rs", &o.cfg_with(&o.features)).unwrap_or_else(|e| die(e));
        let diags = Session2::new(Options2 { no_cache: true, jobs: 1, ..o }).check(&src);
        for d in &diags {
            eprintln!("error: {}:{}:{}: {}", input.display(), d.line, d.col, d.msg);
        }
        std::process::exit(if diags.is_empty() { 0 } else { 1 });
    }
    let output = if output.as_os_str().is_empty() { input.with_extension("") } else { output };
    let mut s = Session2::new(o);
    let mut cp = output.as_os_str().to_owned();
    cp.push(".crcache2");
    let cp = PathBuf::from(cp);
    s.load_cache(&cp);
    match s.build_path(&input, "src/main.rs", &output) {
        Ok(_) => {
            if let Err(e) = s.save_cache(&cp) {
                eprintln!("warning: failed to write cache: {e}");
            }
        }
        Err(e) => die(format!("{}: {e}", input.display())),
    }
}
