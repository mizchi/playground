//! 型の構文 (自前の構文木の `parse::Ty`) を `TySyn` にする: front.rs の `parse_tysyn` (syn 版) の移植。
//! tests/ast_types.rs が全ファイルの型で syn 版と同じ結果になることを確かめる。
//! 外部クレートに依存しない (src/parse.rs と src/types.rs だけ) ので crustc 自身でもコンパイルできる。

use crate::parse::{Bound, Expr, GArg, Path, Stmt, Ty as PTy};
use crate::types::{ArrLen, Ty, TySyn};

pub type R<T> = Result<T, String>;

pub fn ffi_alias(s: &str) -> Option<Ty> {
    Some(match s {
        "c_char" | "c_schar" => Ty::I8,
        "c_uchar" => Ty::U8,
        "c_short" => Ty::I16,
        "c_ushort" => Ty::U16,
        "c_int" => Ty::I32,
        "c_uint" => Ty::U32,
        "c_long" | "c_longlong" => Ty::I64,
        "c_ulong" | "c_ulonglong" => Ty::U64,
        _ => return None,
    })
}

fn lit_u64(t: &str) -> Option<u64> {
    // (`_` を除き、接頭辞で基数を決めて、数字の続く分だけ読む: 接尾辞 `usize` などは捨てる)
    let t: String = t.chars().filter(|&c| c != '_').collect();
    let (radix, body) = if let Some(h) = t.strip_prefix("0x") {
        (16, h)
    } else if let Some(b) = t.strip_prefix("0b") {
        (2, b)
    } else if let Some(o) = t.strip_prefix("0o") {
        (8, o)
    } else {
        (10, t.as_str())
    };
    let digits: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
    u64::from_str_radix(&digits, radix).ok()
}

/// const ジェネリクスの引数 (`Buf<4>` / `Buf<N>` / `Buf<{ 4 }>`): 長さを持つ配列型 `[(); n]`
pub fn const_arg_syn(e: &Expr) -> R<TySyn> {
    match e {
        Expr::Lit(t) if t.starts_with(|c: char| c.is_ascii_digit()) => match lit_u64(t) {
            Some(n) => Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Lit(n))),
            None => Err("bad const argument".to_string()),
        },
        Expr::Block(None, false, b) if b.stmts.len() == 1 => match &b.stmts[0] {
            Stmt::Expr(e, false) => const_arg_syn(e),
            _ => Err("unsupported const argument".to_string()),
        },
        Expr::Path(None, p) if single(p).is_some() => {
            Ok(TySyn::Array(Box::new(TySyn::Prim(Ty::Unit)), ArrLen::Const(single(p).unwrap().to_string())))
        }
        _ => Err("const arguments must be a literal or a name".to_string()),
    }
}

/// 1 つの識別子だけのパス
fn single(p: &Path) -> Option<&str> {
    if !p.global && p.segs.len() == 1 && p.segs[0].args.is_empty() && p.segs[0].fn_args.is_none() {
        Some(&p.segs[0].name)
    } else {
        None
    }
}

pub fn tysyn(t: &PTy, self_ty: Option<&TySyn>) -> R<TySyn> {
    Ok(match t {
        PTy::Never => TySyn::Prim(Ty::Never),
        PTy::Tuple(v) if v.is_empty() => TySyn::Prim(Ty::Unit),
        PTy::Tuple(v) => {
            let mut out = Vec::new();
            for e in v {
                out.push(tysyn(e, self_ty)?);
            }
            TySyn::Tuple(out)
        }
        PTy::Fn(ps, r) => {
            let mut v = Vec::new();
            for a in ps {
                v.push(tysyn(a, self_ty)?);
            }
            v.push(tysyn(r, self_ty)?);
            TySyn::FnPtr(v)
        }
        PTy::Dyn(bs) => {
            let mut tr: Option<String> = None;
            for b in bs {
                match b {
                    // (`dyn Error + Send + Sync`: 自動 trait は読み飛ばす)
                    Bound::Trait(_, p) if p.segs.last().is_some_and(|s| s.name == "Send" || s.name == "Sync") => {}
                    Bound::Trait(_, p) if tr.is_none() => tr = Some(p.segs.last().map(|s| s.text.clone()).unwrap_or_default()),
                    Bound::Lifetime(_) => {}
                    _ => return Err("only one trait is supported in `dyn`".to_string()),
                }
            }
            match tr {
                Some(s) => TySyn::Dyn(s),
                None => return Err("`dyn` needs a trait".to_string()),
            }
        }
        // ライフタイムはコード生成に影響しないので消去する
        PTy::Ref(_, m, i) => TySyn::Ref(*m, Box::new(tysyn(i, self_ty)?)),
        PTy::Ptr(m, i) => TySyn::Ptr(*m, Box::new(tysyn(i, self_ty)?)),
        PTy::Slice(i) => TySyn::Slice(Box::new(tysyn(i, self_ty)?)),
        PTy::Array(e, n) => {
            let len = match &**n {
                Expr::Lit(t) if t.starts_with(|c: char| c.is_ascii_digit()) => match lit_u64(t) {
                    Some(v) => ArrLen::Lit(v),
                    None => return Err("bad array length".to_string()),
                },
                Expr::Path(None, p) if single(p).is_some() => ArrLen::Const(single(p).unwrap().to_string()),
                // 関連定数 `[u8; Buf::CAP]` / `[u8; Self::CAP]`
                Expr::Path(None, p) if !p.global && p.segs.len() == 2 => {
                    let a = p.segs[0].name.clone();
                    let a = match (a.as_str(), self_ty) {
                        ("Self", Some(TySyn::Named(n))) => n.clone(),
                        _ => a,
                    };
                    ArrLen::Const(format!("{a}::{}", p.segs[1].name))
                }
                _ => return Err("array length must be a literal or a const".to_string()),
            };
            TySyn::Array(Box::new(tysyn(e, self_ty)?), len)
        }
        PTy::Infer => TySyn::Infer,
        // `<T as Iterator>::Item`
        PTy::Path(Some(q), p) if !p.segs.is_empty() => {
            TySyn::Assoc(Box::new(tysyn(q, self_ty)?), p.segs.last().unwrap().name.clone())
        }
        PTy::Path(None, p) => path_ty(p, self_ty)?,
        _ => return Err("unsupported type".to_string()),
    })
}

fn no_args(s: &crate::parse::Seg) -> bool {
    s.args.is_empty() && s.fn_args.is_none()
}

fn path_ty(p: &Path, self_ty: Option<&TySyn>) -> R<TySyn> {
    let n = p.segs.len();
    // `Self::Item` / `I::Item` (大文字で始まる 2 セグメントのパス)
    if !p.global && n == 2 && no_args(&p.segs[0]) && no_args(&p.segs[1]) && p.segs[0].name.starts_with(|c: char| c.is_ascii_uppercase()) {
        let b = p.segs[0].name.clone();
        let base = if b == "Self" { self_ty.cloned().unwrap_or(TySyn::Named("Self".into())) } else { TySyn::Named(b) };
        return Ok(TySyn::Assoc(Box::new(base), p.segs[1].name.clone()));
    }
    let last = p.segs.last().ok_or("empty path")?;
    // `io::Error` / `io::Result<T>` (prelude の IoError)
    if n >= 2 && p.segs[n - 2].name == "io" && (last.name == "Result" || last.name == "Error") {
        let io_err = TySyn::Named("IoError".into());
        if last.name == "Error" {
            return Ok(io_err);
        }
        let Some(GArg::Ty(a)) = last.args.first() else {
            return Err("`io::Result` needs a type argument".to_string());
        };
        return Ok(TySyn::App("Result".into(), vec![tysyn(a, self_ty)?, io_err]));
    }
    // `fmt::Result` / `fmt::Error`
    if n >= 2 && p.segs[n - 2].name == "fmt" && (last.name == "Result" || last.name == "Error") {
        let err = TySyn::Named("__FmtError".into());
        if last.name == "Error" {
            return Ok(err);
        }
        return Ok(TySyn::App("Result".into(), vec![TySyn::Prim(Ty::Unit), err]));
    }
    if last.fn_args.is_some() {
        return Err("unsupported type path".to_string());
    }
    let s = last.name.clone();
    let mut generic_args = Vec::new();
    for a in &last.args {
        match a {
            GArg::Ty(t) => {
                // (`Buf<N>`: 型の位置の 1 つの名前は型として読まれる (syn と同じ))
                generic_args.push(tysyn(t, self_ty)?)
            }
            GArg::Lifetime(_) => {}
            GArg::Const(e) => generic_args.push(const_arg_syn(e)?),
            GArg::Bind(..) => return Err("unsupported generic argument".to_string()),
        }
    }
    if !generic_args.is_empty() {
        return Ok(TySyn::App(s, generic_args));
    }
    Ok(match Ty::prim(&s) {
        Some(t) => TySyn::Prim(t),
        None if s == "Self" => match self_ty {
            Some(t) => t.clone(),
            None => return Err("`Self` is only available in impls".to_string()),
        },
        None if s == "str" => TySyn::Str,
        None => match ffi_alias(&s) {
            Some(t) => TySyn::Prim(t),
            None => TySyn::Named(s),
        },
    })
}
