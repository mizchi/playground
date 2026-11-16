//! static の初期値をデータのバイト列 (+ リロケーション) にする定数評価: consteval.rs (syn 版) の移植。
//! 自前の構文木 (src/parse.rs) の上で動き、外部クレートに依存しないので crustc 自身でもコンパイルできる。
//! tests/ast_consteval.rs が全ファイルの static で syn 版と同じ結果になることを確かめる。

use crate::parse::{Expr, Path};
use crate::program::{Program, R};
use crate::types::Ty;

/// データの中から別のデータを指す参照
pub enum DRef {
    /// 無名データ
    Anon(DataVal),
    /// static (Program::consts の index)
    Static(u32),
}

pub struct DataVal {
    pub bytes: Vec<u8>,
    pub align: u32,
    /// (オフセット, 参照先)。ポインタ 1 語 (8 byte) を書く
    pub relocs: Vec<(u32, DRef)>,
}

/// 初期値の式のソースを評価する
pub fn eval_src(prog: &Program, src: &str, ty: Ty) -> R<DataVal> {
    let e = crate::parse::parse_expr(src)?;
    eval(prog, &e, ty)
}

pub fn eval(prog: &Program, e: &Expr, ty: Ty) -> R<DataVal> {
    let (size, align) = prog.size_align(ty);
    let mut v = DataVal { bytes: vec![0; size as usize], align: align.max(1), relocs: vec![] };
    write(prog, e, ty, &mut v, 0, 0)?;
    Ok(v)
}

fn put_int(v: &mut DataVal, off: u32, size: u32, x: i128) {
    let b = x.to_le_bytes();
    v.bytes[off as usize..(off + size) as usize].copy_from_slice(&b[..size as usize]);
}

/// 1 つの識別子だけのパス
fn ident(p: &Path) -> Option<&str> {
    if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() {
        Some(&p.segs[0].name)
    } else {
        None
    }
}

/// 文字列・バイト文字列のリテラル (`"a\n"` / `r#"x"#` / `b"\x01"` / `br"y"`) の中身。(バイト列か, 中身)
pub fn str_lit(t: &str) -> Option<(bool, Vec<u8>)> {
    let (byte, rest) = match t.strip_prefix('b') {
        Some(r) => (true, r),
        None => (false, t),
    };
    if let Some(r) = rest.strip_prefix('r') {
        let hashes = r.len() - r.trim_start_matches('#').len();
        let body = &r[hashes..];
        let body = body.strip_prefix('"')?;
        let body = &body[..body.len().checked_sub(1 + hashes)?];
        return Some((byte, body.as_bytes().to_vec()));
    }
    let body = rest.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = Vec::new();
    let mut cs = body.chars().peekable();
    while let Some(c) = cs.next() {
        if c != '\\' {
            out.extend_from_slice(c.to_string().as_bytes());
            continue;
        }
        match cs.next()? {
            'n' => out.push(b'\n'),
            't' => out.push(b'\t'),
            'r' => out.push(b'\r'),
            '0' => out.push(0),
            '\\' => out.push(b'\\'),
            '\'' => out.push(b'\''),
            '"' => out.push(b'"'),
            'x' => {
                let h: String = [cs.next()?, cs.next()?].iter().collect();
                out.push(u8::from_str_radix(&h, 16).ok()?);
            }
            'u' => {
                let mut h = String::new();
                while let Some(c) = cs.next() {
                    if c == '}' {
                        break;
                    }
                    if c.is_ascii_hexdigit() {
                        h.push(c);
                    }
                }
                let ch = char::from_u32(u32::from_str_radix(&h, 16).ok()?)?;
                out.extend_from_slice(ch.to_string().as_bytes());
            }
            // 行末の `\`: 改行と次の行の先頭の空白を読み飛ばす
            '\n' | '\r' => {
                while cs.peek().is_some_and(|c| c.is_whitespace()) {
                    cs.next();
                }
            }
            _ => return None,
        }
    }
    Some((byte, out))
}

fn write(prog: &Program, e: &Expr, ty: Ty, v: &mut DataVal, off: u32, depth: u32) -> R<()> {
    if depth > 64 {
        return Err("constant is too deeply nested".to_string());
    }
    let mut e = e;
    while let Expr::Paren(x) = e {
        e = x;
    }
    // 他の const / static は名前で辿る
    if let Expr::Path(None, p) = e {
        if let Some(id) = ident(p) {
            if let Some(&ci) = prog.const_index.get(id) {
                let ce = crate::parse::parse_expr(&prog.consts[ci as usize].expr)?;
                return write(prog, &ce, ty, v, off, depth + 1);
            }
        }
    }
    match ty {
        t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => {
            let x = prog.eval_const(e, 0)?;
            put_int(v, off, t.bits() / 8, x);
            Ok(())
        }
        Ty::Array(et, n) => {
            let (es, _) = prog.size_align(*et);
            match e {
                Expr::Array(xs) => {
                    if xs.len() != n as usize {
                        return Err(format!("expected an array with {n} elements"));
                    }
                    for (i, x) in xs.iter().enumerate() {
                        write(prog, x, *et, v, off + i as u32 * es, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Repeat(x, _) => {
                    for i in 0..n {
                        write(prog, x, *et, v, off + i * es, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Lit(t, _) if *et == Ty::U8 && matches!(str_lit(t), Some((true, _))) => {
                    let (_, bs) = str_lit(t).unwrap();
                    v.bytes[off as usize..off as usize + bs.len()].copy_from_slice(&bs);
                    Ok(())
                }
                // (`*b"..."`: バイト列リテラルを配列の値として)
                Expr::Unary(op, x, _) if op == "*" => write(prog, x, ty, v, off, depth + 1),
                Expr::Paren(x) => write(prog, x, ty, v, off, depth + 1),
                _ => Err("unsupported array constant".to_string()),
            }
        }
        Ty::Ref(_, inner) => {
            // 参照: 指す先を無名データにする。&[T] / &str は (ポインタ, 長さ)
            let lit = match e {
                Expr::Lit(t, _) => str_lit(t),
                _ => None,
            };
            let (target, len) = match (e, *inner, lit) {
                (Expr::Lit(_, _), Ty::Str, Some((false, b))) => {
                    let n = b.len();
                    (DRef::Anon(DataVal { bytes: b, align: 1, relocs: vec![] }), Some(n))
                }
                (Expr::Lit(_, _), t, Some((true, b))) => {
                    let n = b.len();
                    let ok = matches!(t, Ty::Slice(Ty::U8)) || matches!(t, Ty::Array(Ty::U8, m) if m as usize == n);
                    if !ok {
                        return Err("mismatched byte string constant".to_string());
                    }
                    (DRef::Anon(DataVal { bytes: b, align: 1, relocs: vec![] }), matches!(t, Ty::Slice(_)).then_some(n))
                }
                (Expr::Ref(_, x), t, _) => {
                    if let Expr::Path(None, p) = &**x {
                        if let Some(&ci) = ident(p).and_then(|id| prog.const_index.get(id)) {
                            if prog.consts[ci as usize].stat.is_some() {
                                let st = prog.consts[ci as usize].ty;
                                let len = match (t, st) {
                                    (Ty::Slice(_), Ty::Array(_, n)) => Some(n as usize),
                                    _ => None,
                                };
                                v.relocs.push((off, DRef::Static(ci)));
                                if let Some(n) = len {
                                    put_int(v, off + 8, 8, n as i128);
                                }
                                return Ok(());
                            }
                        }
                    }
                    match t {
                        Ty::Slice(et) => {
                            let Expr::Array(xs) = &**x else {
                                return Err("slice constants must be `&[..]`".to_string());
                            };
                            let at = Ty::Array(et, xs.len() as u32);
                            (DRef::Anon(eval(prog, x, at)?), Some(xs.len()))
                        }
                        t => (DRef::Anon(eval(prog, x, t)?), None),
                    }
                }
                _ => return Err("unsupported reference constant".to_string()),
            };
            v.relocs.push((off, target));
            if let Some(n) = len {
                put_int(v, off + 8, 8, n as i128);
            }
            Ok(())
        }
        Ty::Struct(sid) => {
            let sd = &prog.structs[sid as usize];
            let vi_of = |p: &Path| -> R<usize> {
                let n = &p.segs.last().ok_or("empty path")?.name;
                sd.variants.iter().position(|x| &x.name == n).ok_or_else(|| format!("no variant `{n}`"))
            };
            let fields_of = |vi: Option<usize>| -> Vec<(String, Ty, u32)> {
                match vi {
                    Some(vi) => sd.variants[vi].fields.clone(),
                    None => sd.fields.clone(),
                }
            };
            let tag_bytes = sd.tag.bits() / 8;
            match e {
                Expr::Struct(p, fvs, _) => {
                    let vi = if sd.is_enum { Some(vi_of(p)?) } else { None };
                    if let Some(vi) = vi {
                        put_int(v, off, tag_bytes, sd.variants[vi].discr as i128);
                    }
                    let fs = fields_of(vi);
                    for (name, x) in fvs {
                        let Some((_, ft, fo)) = fs.iter().find(|f| &f.0 == name) else {
                            return Err(format!("no field `{name}`"));
                        };
                        write(prog, x, *ft, v, off + fo, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Tuple(xs) if !sd.is_enum => {
                    for ((_, ft, fo), x) in sd.fields.iter().zip(xs) {
                        write(prog, x, *ft, v, off + fo, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Call(f, args) => {
                    let Expr::Path(None, p) = &**f else {
                        return Err("unsupported constant".to_string());
                    };
                    let vi = if sd.is_enum { Some(vi_of(p)?) } else { None };
                    if let Some(vi) = vi {
                        put_int(v, off, tag_bytes, sd.variants[vi].discr as i128);
                    }
                    for ((_, ft, fo), x) in fields_of(vi).iter().zip(args) {
                        write(prog, x, *ft, v, off + fo, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Path(None, p) if sd.is_enum => {
                    let vi = vi_of(p)?;
                    put_int(v, off, tag_bytes, sd.variants[vi].discr as i128);
                    Ok(())
                }
                // unit struct (`static K: Key = Key;`): 中身はない
                Expr::Path(None, _) if !sd.is_enum && sd.fields.is_empty() => Ok(()),
                _ => Err("unsupported struct / enum constant".to_string()),
            }
        }
        Ty::Unit => Ok(()),
        t => Err(format!("constants of type `{}` are not supported", prog.tn(t))),
    }
}

/// データ (と中の参照先) を 1 行ずつの文字列にする (テスト・セルフホストの比較用)
pub fn dump(dv: &DataVal, indent: usize, out: &mut Vec<String>) {
    let hex: String = dv.bytes.iter().map(|b| format!("{b:02x}")).collect();
    out.push(format!("{}align={} {hex}", " ".repeat(indent), dv.align));
    for (o, r) in &dv.relocs {
        match r {
            DRef::Static(c) => out.push(format!("{}@{o} static {c}", " ".repeat(indent + 1))),
            DRef::Anon(inner) => {
                out.push(format!("{}@{o} anon", " ".repeat(indent + 1)));
                dump(inner, indent + 2, out);
            }
        }
    }
}
