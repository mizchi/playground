//! リンク。
//!
//! `cc` ドライバ経由だと毎回 gcc driver → collect2 → ld と 3 段プロセスが立ち上がる。
//! 初回だけ `cc -###` でドライバが組み立てる ld のコマンドラインを取得して
//! `<out>.link` に保存し、以後はリンカ (wild / ld.lld / ld) を直接 exec する。
//! (1000 関数の例で cc+lld 21ms → wild 直接 5ms)

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn which(name: &str) -> Option<PathBuf> {
    if Path::new(name).is_absolute() {
        return Some(name.into());
    }
    let p = std::env::var_os("PATH")?;
    std::env::split_paths(&p)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// 使うリンカの実行ファイル。既定は wild > mold > ld.lld > ld。
pub fn pick(requested: Option<&str>) -> Result<PathBuf, String> {
    let cands: Vec<&str> = match requested {
        Some("lld") => vec!["ld.lld"],
        Some("bfd") => vec!["ld.bfd", "ld"],
        Some(l) => vec![l],
        None => vec!["wild", "mold", "ld.lld", "ld"],
    };
    cands
        .iter()
        .find_map(|c| which(c))
        .ok_or_else(|| format!("linker not found: {cands:?}"))
}

const DRIVER_FLAGS: &[&str] = &["-no-pie", "-s", "-Wl,--gc-sections", "-Wl,--build-id=none"];

/// `cc -###` の出力から collect2 の引数を取り出し、プラグイン関連を除いたものを返す。
fn driver_args(obj: &Path, out: &Path) -> Result<Vec<String>, String> {
    let o = Command::new("cc")
        .arg("-###")
        .arg(obj)
        .arg("-o")
        .arg(out)
        .args(DRIVER_FLAGS)
        .output()
        .map_err(|e| format!("failed to run cc: {e}"))?;
    let text = String::from_utf8_lossy(&o.stderr);
    let line = text
        .lines()
        .find(|l| l.contains("collect2") || l.trim_start().starts_with("\"/") && l.contains(" -o "))
        .ok_or("could not find the link line in `cc -###` output")?;
    let mut args = vec![];
    let mut skip = false;
    for (i, a) in shell_split(line).into_iter().enumerate() {
        if i == 0 || std::mem::take(&mut skip) {
            continue;
        }
        if a == "-plugin" {
            skip = true;
            continue;
        }
        if a.starts_with("-plugin-opt") || a.starts_with("-fuse-ld") {
            continue;
        }
        args.push(a);
    }
    Ok(args)
}

/// `cc -###` の出力は `"..."` で囲まれた引数と裸の引数が混ざっている。
fn shell_split(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_q = false;
    let mut any = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_q = !in_q;
                any = true;
            }
            '\\' if in_q => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !in_q => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn link(requested: Option<&str>, obj: &Path, out: &Path) -> Result<String, String> {
    let linker = pick(requested)?;
    let mut lpath = out.as_os_str().to_owned();
    lpath.push(".link");
    let lpath = PathBuf::from(lpath);
    // 保存したリンク行は (obj, out) のパスにだけ依存する
    let header = format!("{}\n{}\n", obj.display(), out.display());
    let args = match std::fs::read_to_string(&lpath) {
        Ok(s) if s.starts_with(&header) => s[header.len()..].lines().map(String::from).collect(),
        _ => {
            let a = driver_args(obj, out)?;
            let _ = std::fs::write(&lpath, format!("{header}{}", a.join("\n")));
            a
        }
    };
    let st = Command::new(&linker)
        .args(&args)
        .status()
        .map_err(|e| format!("failed to run {}: {e}", linker.display()))?;
    if !st.success() {
        let _ = std::fs::remove_file(&lpath);
        return Err(format!("{} failed: {st}", linker.display()));
    }
    Ok(linker.file_name().unwrap().to_string_lossy().into_owned())
}
