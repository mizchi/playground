//! 複数ファイルのクレート: `mod x;` を `x.rs` (か `x/mod.rs`) の中身で置き換えて 1 つのソースにし、
//! 結合したソースの行 → (ファイル, ファイルの中の行) の対応 (SrcMap) を作る。
//!
//! 行番号をずらさないため、`mod x;` は同じ長さの空白にして、中身はファイルの最後に `pub mod x { .. }` として足す
//! (Rust ではアイテムの順は意味を持たない)。コンパイルエラーの `行:桁:` と panic の場所はこの対応で元のファイルに戻す。

use std::path::{Path, PathBuf};

/// 結合したソースの行の区間: (開始行 (1 始まり), ファイルの番号, そのファイルでの開始行)
#[derive(Clone, Debug, Default)]
pub struct SrcMap {
    pub segs: Vec<(u32, u32, u32)>,
    /// 表示するファイルの名前 (`src/main.rs`, `src/foo/bar.rs` など)
    pub files: Vec<String>,
    /// 読んだファイル
    pub inputs: Vec<PathBuf>,
    /// モジュールの名前を変えて桁がずれた所: (行, 桁, ずれ) (lexer::rename_module_collisions_fix)
    pub colfix: Vec<(u32, u32, i32)>,
}

impl SrcMap {
    /// 1 ファイルだけ (区間 1 つ)
    pub fn single(name: &str) -> SrcMap {
        SrcMap { segs: vec![(1, 0, 1)], files: vec![name.to_string()], inputs: vec![], colfix: vec![] }
    }

    /// 結合したソースの行 → (ファイルの名前, ファイルの中の行)
    pub fn locate(&self, line: u32) -> (&str, u32) {
        let mut best = (0u32, 1u32, 1u32);
        for &(s, f, first) in &self.segs {
            if s <= line {
                best = (f, s, first);
            }
        }
        let name = self.files.get(best.0 as usize).map(|s| s.as_str()).unwrap_or("src/main.rs");
        (name, line - best.1 + best.2)
    }

    /// 結合したソースの (行, 桁) の桁を、名前を変える前の桁にする
    pub fn fix_col(&self, line: u32, col: u32) -> u32 {
        let mut d = 0;
        for &(l, c, cum) in &self.colfix {
            if l == line && c <= col {
                d = cum;
            }
        }
        (col as i64 - d as i64).max(0) as u32
    }

    /// エラーの先頭の `行:桁:` (か `行:`) を `ファイル:行:桁:` にする
    pub fn relocate_err(&self, e: &str) -> String {
        let b = e.as_bytes();
        let mut i = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == 0 || i >= b.len() || b[i] != b':' {
            return e.to_string();
        }
        let line: u32 = e[..i].parse().unwrap_or(0);
        let (f, l) = self.locate(line);
        // (桁)
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 && j < b.len() && b[j] == b':' {
            let col: u32 = e[i + 1..j].parse().unwrap_or(0);
            return format!("{f}:{l}:{}{}", self.fix_col(line, col), &e[j..]);
        }
        format!("{f}:{l}{}", &e[i..])
    }
}

fn line_count(s: &str) -> u32 {
    s.as_bytes().iter().filter(|&&c| c == b'\n').count() as u32
}

/// クレートの根のファイル (main.rs / lib.rs) を読み、`mod x;` を展開する。display は根の表示名 (`src/main.rs` など)
pub fn load_crate(root: &Path, display: &str) -> Result<(String, SrcMap), String> {
    let mut map = SrcMap::default();
    let dir = root.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let disp_dir = match display.rfind('/') {
        Some(i) => display[..i].to_string(),
        None => String::new(),
    };
    let text = load_file(root, display, dir.as_path(), &disp_dir, 1, &mut map)?;
    Ok((text, map))
}

/// ファイル path を読んで展開する。mod_dir は子のモジュールのファイルを探すディレクトリ。start は結合したソースでの開始行
fn load_file(path: &Path, display: &str, mod_dir: &Path, disp_dir: &str, start: u32, map: &mut SrcMap) -> Result<String, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    map.inputs.push(path.to_path_buf());
    let src = match crate::macros::expand_includes(&src, path.parent().unwrap_or(Path::new("."))) {
        Ok(Some(s)) => s,
        Ok(None) => src,
        Err(e) => return Err(format!("{display}: {e}")),
    };
    let fi = map.files.len() as u32;
    map.files.push(display.to_string());
    map.segs.push((start, fi, 1));
    let (mut text, kids) = blank_mod_decls(&src).map_err(|e| format!("{display}: {e}"))?;
    if kids.is_empty() {
        return Ok(text);
    }
    text.push('\n');
    for (name, is_cfg_test) in kids {
        let _ = is_cfg_test;
        let a = mod_dir.join(format!("{name}.rs").as_str());
        let b = mod_dir.join(name.as_str()).join("mod.rs");
        let (p, disp, sub, sub_disp) = if a.exists() {
            (a, join_disp(disp_dir, &format!("{name}.rs")), mod_dir.join(name.as_str()), join_disp(disp_dir, &name))
        } else if b.exists() {
            (b, join_disp(disp_dir, &format!("{name}/mod.rs")), mod_dir.join(name.as_str()), join_disp(disp_dir, &name))
        } else {
            return Err(format!("{display}: file not found for module `{name}`"));
        };
        text.push_str(&format!("pub mod {name} {{\n"));
        let at = start + line_count(&text);
        let body = load_file(p.as_path(), &disp, sub.as_path(), &sub_disp, at, map)?;
        text.push_str(&body);
        text.push_str("\n}\n");
        // (`}` の行は、元のファイルの最後の行の続きにしておく)
        map.segs.push((start + line_count(&text) - 1, fi, 1 + line_count(&src)));
    }
    Ok(text)
}

fn join_disp(dir: &str, rest: &str) -> String {
    if dir.is_empty() { rest.to_string() } else { format!("{dir}/{rest}") }
}

/// トップレベルの `[pub[(..)]] mod x;` を空白にし、(名前, #[cfg(test)] か) の列を返す。`#[cfg(test)]` のものは読まない
fn blank_mod_decls(src: &str) -> Result<(String, Vec<(String, bool)>), String> {
    if !src.contains("mod") {
        return Ok((src.to_string(), vec![]));
    }
    let s = src.as_bytes();
    let toks = crate::lexer::tokenize(s);
    let tok = |k: usize| -> &[u8] { &s[toks[k].1..toks[k].2] };
    let is_sym = |k: usize, c: u8| toks[k].0 == 1 && toks[k].2 == toks[k].1 + 1 && s[toks[k].1] == c;
    let mut out = s.to_vec();
    let mut kids = vec![];
    let mut depth = 0i32;
    let mut k = 0;
    while k < toks.len() {
        if is_sym(k, b'{') {
            depth += 1;
        } else if is_sym(k, b'}') {
            depth -= 1;
        } else if toks[k].0 == 0 && tok(k) == b"mod" && k + 2 < toks.len() && toks[k + 1].0 == 0 && is_sym(k + 2, b';') {
            if depth != 0 {
                return Err(format!("`mod {};` inside a block is not supported", String::from_utf8_lossy(tok(k + 1))));
            }
            // (前の `pub` / `pub(crate)` と `#[..]` も消す)
            let mut b = k;
            if b >= 1 && is_sym(b - 1, b')') {
                let mut j = b - 1;
                while j > 0 && !is_sym(j, b'(') {
                    j -= 1;
                }
                if j >= 1 && toks[j - 1].0 == 0 && tok(j - 1) == b"pub" {
                    b = j - 1;
                }
            } else if b >= 1 && toks[b - 1].0 == 0 && tok(b - 1) == b"pub" {
                b -= 1;
            }
            let mut cfg_test = false;
            while b >= 1 && is_sym(b - 1, b']') {
                let mut j = b - 1;
                while j > 0 && !is_sym(j, b'[') {
                    j -= 1;
                }
                if j >= 1 && is_sym(j - 1, b'#') {
                    let attr = &src[toks[j].1..toks[b - 1].2];
                    if attr.contains("cfg") && attr.contains("test") {
                        cfg_test = true;
                    }
                    b = j - 1;
                } else {
                    break;
                }
            }
            for c in &mut out[toks[b].1..toks[k + 2].2] {
                if *c != b'\n' {
                    *c = b' ';
                }
            }
            if !cfg_test {
                kids.push((String::from_utf8_lossy(tok(k + 1)).into_owned(), false));
            }
            k += 3;
            continue;
        }
        k += 1;
    }
    Ok((String::from_utf8(out).map_err(|e| e.to_string())?, kids))
}
