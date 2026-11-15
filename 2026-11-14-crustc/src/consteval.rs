//! static の初期値を、データセクションに置くバイト列 (+ リロケーション) にする定数評価。
//! 対応: 整数 / bool / char (定数式)、配列 (列挙・`[x; N]`)、struct / タプル / enum の値、
//! `b"..."` / `"..."` / `&[..]` / `&e` (別のデータへの参照)、他の const / static への参照。

use syn::spanned::Spanned;
use syn::{Expr, Lit};

use crate::front::{Program, R, Ty, err};

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

pub fn eval(prog: &Program, e: &Expr, ty: Ty) -> R<DataVal> {
    let (size, align) = prog.size_align(ty);
    let mut v = DataVal {
        bytes: vec![0; size as usize],
        align: align.max(1),
        relocs: vec![],
    };
    write(prog, e, ty, &mut v, 0, 0)?;
    Ok(v)
}

fn put_int(v: &mut DataVal, off: u32, size: u32, x: i128) {
    let b = x.to_le_bytes();
    v.bytes[off as usize..(off + size) as usize].copy_from_slice(&b[..size as usize]);
}

fn write(prog: &Program, e: &Expr, ty: Ty, v: &mut DataVal, off: u32, depth: u32) -> R<()> {
    if depth > 64 {
        return err(e.span(), "constant is too deeply nested");
    }
    let e = match e {
        Expr::Paren(p) => &*p.expr,
        Expr::Group(g) => &*g.expr,
        e => e,
    };
    // 他の const / static は名前で辿る
    if let Expr::Path(p) = e {
        if let Some(id) = p.path.get_ident() {
            if let Some(&ci) = prog.const_index.get(&id.to_string()) {
                let c = &prog.consts[ci as usize];
                let ce: Expr = syn::parse_str(&c.expr).or_else(|er| err(e.span(), er))?;
                return write(prog, &ce, ty, v, off, depth + 1);
            }
        }
    }
    match ty {
        t if t.is_int() || matches!(t, Ty::Bool | Ty::Char) => {
            let x = prog.eval_int(e, 0).or_else(|er| err(e.span(), er))?;
            put_int(v, off, t.bits() / 8, x);
            Ok(())
        }
        Ty::Array(et, n) => {
            let (es, _) = prog.size_align(*et);
            match e {
                Expr::Array(a) => {
                    if a.elems.len() != n as usize {
                        return err(a.span(), format!("expected an array with {n} elements"));
                    }
                    for (i, x) in a.elems.iter().enumerate() {
                        write(prog, x, *et, v, off + i as u32 * es, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Repeat(r) => {
                    for i in 0..n {
                        write(prog, &r.expr, *et, v, off + i * es, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Lit(syn::ExprLit {
                    lit: Lit::ByteStr(b),
                    ..
                }) if *et == Ty::U8 => {
                    let bs = b.value();
                    v.bytes[off as usize..off as usize + bs.len()].copy_from_slice(&bs);
                    Ok(())
                }
                _ => err(e.span(), "unsupported array constant"),
            }
        }
        Ty::Ref(_, inner) => {
            // 参照: 指す先を無名データにする。&[T] / &str は (ポインタ, 長さ)
            let (target, len) = match (e, *inner) {
                (
                    Expr::Lit(syn::ExprLit {
                        lit: Lit::Str(s), ..
                    }),
                    Ty::Str,
                ) => {
                    let b = s.value().into_bytes();
                    let n = b.len();
                    (
                        DRef::Anon(DataVal {
                            bytes: b,
                            align: 1,
                            relocs: vec![],
                        }),
                        Some(n),
                    )
                }
                (
                    Expr::Lit(syn::ExprLit {
                        lit: Lit::ByteStr(s),
                        ..
                    }),
                    t,
                ) => {
                    let b = s.value();
                    let n = b.len();
                    let ok = matches!(t, Ty::Slice(Ty::U8))
                        || matches!(t, Ty::Array(Ty::U8, m) if m as usize == n);
                    if !ok {
                        return err(e.span(), "mismatched byte string constant");
                    }
                    (
                        DRef::Anon(DataVal {
                            bytes: b,
                            align: 1,
                            relocs: vec![],
                        }),
                        matches!(t, Ty::Slice(_)).then_some(n),
                    )
                }
                (Expr::Reference(r), t) => {
                    if let Expr::Path(p) = &*r.expr {
                        if let Some(&ci) = p
                            .path
                            .get_ident()
                            .and_then(|id| prog.const_index.get(&id.to_string()))
                        {
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
                            let Expr::Array(a) = &*r.expr else {
                                return err(r.span(), "slice constants must be `&[..]`");
                            };
                            let at = Ty::Array(et, a.elems.len() as u32);
                            (DRef::Anon(eval(prog, &r.expr, at)?), Some(a.elems.len()))
                        }
                        t => (DRef::Anon(eval(prog, &r.expr, t)?), None),
                    }
                }
                _ => return err(e.span(), "unsupported reference constant"),
            };
            v.relocs.push((off, target));
            if let Some(n) = len {
                put_int(v, off + 8, 8, n as i128);
            }
            Ok(())
        }
        Ty::Struct(sid) => {
            let sd = &prog.structs[sid as usize];
            let fields_of = |vi: Option<usize>| match vi {
                Some(vi) => sd.variants[vi].fields.clone(),
                None => sd.fields.clone(),
            };
            let set_tag = |v: &mut DataVal, vi: usize| {
                put_int(v, off, sd.tag.bits() / 8, sd.variants[vi].discr as i128)
            };
            let vi_of = |p: &syn::Path| -> R<usize> {
                let n = p.segments.last().unwrap().ident.to_string();
                sd.variants
                    .iter()
                    .position(|x| x.name == n)
                    .ok_or_else(|| format!("no variant `{n}`"))
            };
            match e {
                Expr::Struct(s) => {
                    let vi = if sd.is_enum {
                        Some(vi_of(&s.path)?)
                    } else {
                        None
                    };
                    if let Some(vi) = vi {
                        set_tag(v, vi);
                    }
                    let fs = fields_of(vi);
                    for fv in &s.fields {
                        let name = match &fv.member {
                            syn::Member::Named(id) => id.to_string(),
                            syn::Member::Unnamed(i) => i.index.to_string(),
                        };
                        let Some((_, ft, fo)) = fs.iter().find(|f| f.0 == name) else {
                            return err(fv.member.span(), format!("no field `{name}`"));
                        };
                        write(prog, &fv.expr, *ft, v, off + fo, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Tuple(t) if !sd.is_enum => {
                    for ((_, ft, fo), x) in sd.fields.iter().zip(&t.elems) {
                        write(prog, x, *ft, v, off + fo, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Call(c) => {
                    let Expr::Path(p) = &*c.func else {
                        return err(c.span(), "unsupported constant");
                    };
                    let vi = if sd.is_enum {
                        Some(vi_of(&p.path)?)
                    } else {
                        None
                    };
                    if let Some(vi) = vi {
                        set_tag(v, vi);
                    }
                    for ((_, ft, fo), x) in fields_of(vi).iter().zip(&c.args) {
                        write(prog, x, *ft, v, off + fo, depth + 1)?;
                    }
                    Ok(())
                }
                Expr::Path(p) if sd.is_enum => {
                    set_tag(v, vi_of(&p.path)?);
                    Ok(())
                }
                _ => err(e.span(), "unsupported struct / enum constant"),
            }
        }
        Ty::Unit => Ok(()),
        t => err(
            e.span(),
            format!("constants of type `{}` are not supported", prog.tn(t)),
        ),
    }
}
