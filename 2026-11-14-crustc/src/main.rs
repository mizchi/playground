//! exrustc の CLI。
//!   exrustc <input.rs> [-o out] [...]   ビルド
//!   exrustc check <input.rs>            型検査だけ (全診断を表示)
//!   exrustc lsp                         LSP サーバ (stdio)

use std::path::PathBuf;

use exrustc::{Options, Session, cache_path};

fn usage() -> ! {
    eprintln!(
        "usage: exrustc <input.rs> [-o out] [-O] [--regalloc=fast|good] [--linker=NAME] [--time] [-j N] [--no-cache]\n       exrustc check <input.rs>\n       exrustc lsp"
    );
    std::process::exit(2)
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let mut o = Options::default();
    let mut input = PathBuf::new();
    let mut output = PathBuf::new();
    let mut mode = "build";
    match args.peek().map(String::as_str) {
        Some("check") => {
            mode = "check";
            args.next();
        }
        Some("lsp") => {
            std::process::exit(
                match exrustc::lsp::run(std::io::stdin().lock(), std::io::stdout().lock()) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("exrustc lsp: {e}");
                        1
                    }
                },
            );
        }
        _ => {}
    }
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => output = args.next().unwrap_or_else(|| usage()).into(),
            "-O" => {
                o.opt = true;
                o.fast_regalloc = false;
            }
            "--regalloc=fast" => o.fast_regalloc = true,
            "--regalloc=good" => o.fast_regalloc = false,
            "--time" => o.time = true,
            "--no-cache" => o.no_cache = true,
            "-j" => {
                o.jobs = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            s if s.starts_with("--linker=") => o.linker = Some(s["--linker=".len()..].into()),
            s if s.starts_with('-') => usage(),
            s => input = s.into(),
        }
    }
    if input.as_os_str().is_empty() {
        usage();
    }
    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {}: {e}", input.display());
            std::process::exit(1);
        }
    };
    let mut s = Session::new(o);
    if mode == "check" {
        let diags = s.check(&src);
        for d in &diags {
            eprintln!("error: {}:{d}", input.display());
        }
        std::process::exit(if diags.is_empty() { 0 } else { 1 });
    }
    if output.as_os_str().is_empty() {
        output = input.with_extension("");
    }
    let cp = cache_path(&output);
    s.load_cache(&cp);
    let r = s.build(&src, &output);
    match r {
        Ok(_) => {
            if let Err(e) = s.save_cache(&cp) {
                eprintln!("warning: failed to write cache: {e}");
            }
        }
        Err(e) => {
            eprintln!("error: {}: {e}", input.display());
            std::process::exit(1);
        }
    }
}
