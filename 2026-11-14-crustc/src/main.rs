//! crustc の CLI。
//!   crustc <input.rs> [-o out] [...]   ビルド
//!   crustc check <input.rs>            型検査だけ (全診断を表示)
//!   crustc build <workspace> [-o dir]   ワークスペース (複数のクレート) をビルド (`--features acg`)
//!   crustc lsp                         LSP サーバ (stdio)

use std::path::PathBuf;

use crustc::{Options, Session, cache_path};

fn usage() -> ! {
    eprintln!(
        "usage: crustc <input.rs> [-o out] [-O] [--regalloc=fast|good] [--linker=NAME] [--time] [-j N] [--no-cache]\n       crustc check <input.rs>\n       crustc build <workspace-file> [-o out-dir] [-O] [-j N] [--time] [--no-cache]\n       crustc lsp"
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
        Some("build") => {
            mode = "workspace";
            args.next();
        }
        Some("lsp") => {
            std::process::exit(
                match crustc::lsp::run(std::io::stdin().lock(), std::io::stdout().lock()) {
                    Ok(()) => 0,
                    Err(e) => {
                        eprintln!("crustc lsp: {e}");
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
    #[cfg(feature = "acg")]
    if mode == "workspace" {
        let crates = match crustc::driver2::read_workspace(&input) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        };
        let out_dir = if output.as_os_str().is_empty() { input.parent().map(|d| d.join("target")).unwrap_or("target".into()) } else { output.clone() };
        let opts = crustc::driver2::Options2 { opt: o.opt, jobs: o.jobs, linker: o.linker.clone(), no_cache: o.no_cache, time: o.time };
        let mut s = crustc::driver2::Session2::new(opts);
        let cp = out_dir.join("workspace.crcache2");
        s.load_cache(&cp);
        match s.build_workspace(&crates, &out_dir) {
            Ok(_) => {
                if let Err(e) = s.save_cache(&cp) {
                    eprintln!("warning: failed to write cache: {e}");
                }
                std::process::exit(0)
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
    let src = match std::fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {}: {e}", input.display());
            std::process::exit(1);
        }
    };
    // `include_str!("..")` / `include_bytes!("..")`: 入力ファイルのディレクトリから読んでリテラルに
    let dir = input.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let src = match crustc::macros::expand_includes(&src, &dir) {
        Ok(Some(s)) => s,
        Ok(None) => src,
        Err(e) => {
            eprintln!("error: {}: {e}", input.display());
            std::process::exit(1);
        }
    };
    // (CRUSTC_DRIVER2=1: syn・Cranelift を使わないドライバでビルドする。`--features acg` のときだけ)
    #[cfg(feature = "acg")]
    if std::env::var_os("CRUSTC_DRIVER2").is_some() && mode != "check" {
        let output = if output.as_os_str().is_empty() { input.with_extension("") } else { output.clone() };
        let opts = crustc::driver2::Options2 { opt: o.opt, jobs: o.jobs, linker: o.linker.clone(), no_cache: o.no_cache, time: o.time };
        let mut s = crustc::driver2::Session2::new(opts);
        let mut cp = output.as_os_str().to_owned();
        cp.push(".crcache2");
        let cp = std::path::PathBuf::from(cp);
        s.load_cache(&cp);
        let _ = &src;
        match s.build_path(&input, "src/main.rs", &output) {
            Ok(_) => {
                if let Err(e) = s.save_cache(&cp) {
                    eprintln!("warning: failed to write cache: {e}");
                }
                std::process::exit(0)
            }
            Err(e) => {
                eprintln!("error: {}: {e}", input.display());
                std::process::exit(1);
            }
        }
    }
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
