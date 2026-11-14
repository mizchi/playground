//! フロントエンド: syn の AST からシグネチャ表と呼び出しグラフを作る。
//!
//! ここは「関数本体を見ない」軽い処理だけにしている。本体の型検査・lowering は
//! codegen 側で関数ごとに行い、キャッシュヒットした関数は本体に一切触れない。

use std::collections::HashMap;

use syn::spanned::Spanned;

use crate::lexer::SrcItem;

pub type R<T> = Result<T, String>;

pub fn err<T>(span: proc_macro2::Span, msg: impl std::fmt::Display) -> R<T> {
    let s = span.start();
    Err(format!("{}:{}: {}", s.line, s.column + 1, msg))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Ty {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
    Bool,
    Unit,
    Never,
}

impl Ty {
    pub fn parse(t: &syn::Type) -> R<Ty> {
        Ok(match t {
            syn::Type::Never(_) => Ty::Never,
            syn::Type::Tuple(tt) if tt.elems.is_empty() => Ty::Unit,
            syn::Type::Paren(p) => Ty::parse(&p.elem)?,
            syn::Type::Path(p) if p.qself.is_none() => {
                let Some(id) = p.path.get_ident() else {
                    return err(t.span(), "unsupported type path");
                };
                match id.to_string().as_str() {
                    "i8" => Ty::I8,
                    "i16" => Ty::I16,
                    "i32" => Ty::I32,
                    "i64" => Ty::I64,
                    "isize" => Ty::Isize,
                    "u8" => Ty::U8,
                    "u16" => Ty::U16,
                    "u32" => Ty::U32,
                    "u64" => Ty::U64,
                    "usize" => Ty::Usize,
                    "bool" => Ty::Bool,
                    other => return err(t.span(), format!("unsupported type `{other}`")),
                }
            }
            _ => return err(t.span(), "unsupported type"),
        })
    }

    pub fn from_suffix(s: &str) -> Option<Ty> {
        Some(match s {
            "i8" => Ty::I8,
            "i16" => Ty::I16,
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "isize" => Ty::Isize,
            "u8" => Ty::U8,
            "u16" => Ty::U16,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "usize" => Ty::Usize,
            _ => return None,
        })
    }

    pub fn is_int(self) -> bool {
        !matches!(self, Ty::Bool | Ty::Unit | Ty::Never)
    }

    pub fn signed(self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::Isize)
    }

    pub fn bits(self) -> u32 {
        match self {
            Ty::I8 | Ty::U8 | Ty::Bool => 8,
            Ty::I16 | Ty::U16 => 16,
            Ty::I32 | Ty::U32 => 32,
            Ty::I64 | Ty::U64 | Ty::Isize | Ty::Usize => 64,
            Ty::Unit | Ty::Never => 0,
        }
    }
}

#[derive(Clone, Debug, Hash)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// オブジェクトファイル上のシンボル名
    pub symbol: String,
    pub is_extern: bool,
    pub is_main: bool,
}

/// アイテム 1 個分の要約 (キャッシュされる)。ここに入っている関数のシグネチャだけで
/// 他の関数のコード生成ができる。
#[derive(Clone, Debug)]
pub struct Summary {
    /// (名前, シグネチャ)。fn アイテムなら 1 個、extern ブロックなら複数
    pub fns: Vec<(String, FnSig)>,
}

pub struct Program {
    /// 全関数 (ユーザ定義 + extern)。index が u32 の関数 ID になる
    pub names: Vec<String>,
    pub index: HashMap<String, u32>,
    pub sigs: Vec<FnSig>,
    /// ユーザ定義関数を定義しているアイテム番号 (extern は None)
    pub item_of: Vec<Option<u32>>,
}

/// syn で 1 アイテムを parse する。エラー位置はファイル全体での行に補正する。
pub fn parse_item(src: &str, it: &SrcItem) -> R<syn::Item> {
    syn::parse_str::<syn::Item>(&src[it.start..it.end]).map_err(|e| {
        let s = e.span().start();
        format!("{}:{}: {e}", s.line + it.line - 1, s.column + 1)
    })
}

/// "L:C: msg" 形式のエラーの行をアイテム開始行だけずらす。
pub fn relocate_err(e: String, it: &SrcItem) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    e
}

pub fn summarize(item: &syn::Item) -> R<Summary> {
    let mut fns = vec![];
    match item {
        syn::Item::Fn(f) => {
            let name = f.sig.ident.to_string();
            let is_main = name == "main";
            let sig = parse_sig(&f.sig, false, is_main)?;
            if is_main && (!sig.params.is_empty() || sig.ret != Ty::Unit) {
                return err(f.sig.span(), "`main` must be `fn main()`");
            }
            fns.push((name, sig));
        }
        syn::Item::ForeignMod(m) => {
            for fi in &m.items {
                let syn::ForeignItem::Fn(f) = fi else {
                    return err(fi.span(), "only fns are supported in extern blocks");
                };
                fns.push((f.sig.ident.to_string(), parse_sig(&f.sig, true, false)?));
            }
        }
        _ => return err(item.span(), "unsupported item"),
    }
    Ok(Summary { fns })
}

impl Program {
    pub fn build(items: &[SrcItem], sums: &[Summary]) -> R<Program> {
        let mut p = Program { names: vec![], index: HashMap::new(), sigs: vec![], item_of: vec![] };
        for (ii, (it, sm)) in items.iter().zip(sums).enumerate() {
            for (name, sig) in &sm.fns {
                if p.index.contains_key(name) {
                    return Err(format!("{}:1: duplicate definition of `{name}`", it.line));
                }
                p.index.insert(name.clone(), p.names.len() as u32);
                p.names.push(name.clone());
                p.item_of.push((!sig.is_extern).then_some(ii as u32));
                p.sigs.push(sig.clone());
            }
        }
        if !p.index.contains_key("main") {
            return Err("no `main` function".into());
        }
        Ok(p)
    }

    /// アイテム内の識別子のうち関数名に一致するもの (ソート済み)。
    /// ローカル変数による shadowing は無視した過大近似だが、
    /// キャッシュキーと到達可能性の両方にとって安全側である。
    pub fn refs(&self, src: &str, it: &SrcItem) -> Vec<u32> {
        let mut out: Vec<u32> = it
            .idents
            .iter()
            .filter_map(|&(a, b)| self.index.get(&src[a as usize..b as usize]).copied())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// main から到達可能な関数 ID。
    /// 到達しない関数は parse も型検査もコード生成もしない (relax/blaze の demand-driven の AOT 版)。
    pub fn reachable(&self, refs: &[Vec<u32>]) -> Vec<bool> {
        let mut seen = vec![false; self.names.len()];
        let mut stack = vec![self.index["main"]];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i as usize], true) {
                continue;
            }
            stack.extend(refs[i as usize].iter().copied());
        }
        seen
    }
}

// --- Summary のシリアライズ ------------------------------------------------

const TYS: [Ty; 13] = [
    Ty::I8, Ty::I16, Ty::I32, Ty::I64, Ty::Isize, Ty::U8, Ty::U16, Ty::U32, Ty::U64, Ty::Usize,
    Ty::Bool, Ty::Unit, Ty::Never,
];

fn ty_code(t: Ty) -> u8 {
    TYS.iter().position(|&x| x == t).unwrap() as u8
}

impl Summary {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.fns.len() as u8);
        for (name, s) in &self.fns {
            for st in [name, &s.symbol] {
                out.push(st.len() as u8);
                out.extend_from_slice(st.as_bytes());
            }
            out.push(s.is_extern as u8 | (s.is_main as u8) << 1);
            out.push(ty_code(s.ret));
            out.push(s.params.len() as u8);
            out.extend(s.params.iter().map(|&t| ty_code(t)));
        }
    }

    pub fn decode(d: &[u8]) -> Option<Summary> {
        let mut i = 0;
        let mut next = |n: usize| -> Option<&[u8]> {
            let r = d.get(i..i + n)?;
            i += n;
            Some(r)
        };
        let nf = next(1)?[0];
        let mut fns = vec![];
        for _ in 0..nf {
            let mut strs = vec![];
            for _ in 0..2 {
                let l = next(1)?[0] as usize;
                strs.push(String::from_utf8(next(l)?.to_vec()).ok()?);
            }
            let fl = next(1)?[0];
            let ret = *TYS.get(next(1)?[0] as usize)?;
            let np = next(1)?[0] as usize;
            let params = next(np)?.iter().map(|&c| TYS.get(c as usize).copied()).collect::<Option<_>>()?;
            let symbol = strs.pop()?;
            let name = strs.pop()?;
            fns.push((name, FnSig { params, ret, symbol, is_extern: fl & 1 != 0, is_main: fl & 2 != 0 }));
        }
        Some(Summary { fns })
    }
}

fn parse_sig(sig: &syn::Signature, is_extern: bool, is_main: bool) -> R<FnSig> {
    if !sig.generics.params.is_empty() {
        return err(sig.generics.span(), "generics are not supported yet");
    }
    if sig.variadic.is_some() {
        return err(sig.span(), "variadic functions are not supported");
    }
    let mut params = vec![];
    for a in &sig.inputs {
        match a {
            syn::FnArg::Typed(pt) => params.push(Ty::parse(&pt.ty)?),
            syn::FnArg::Receiver(r) => return err(r.span(), "methods are not supported"),
        }
    }
    let ret = match &sig.output {
        syn::ReturnType::Default => Ty::Unit,
        syn::ReturnType::Type(_, t) => Ty::parse(t)?,
    };
    let name = sig.ident.to_string();
    let symbol = if is_extern || is_main {
        name
    } else {
        // ローカルシンボル。短く保つことで .o / シンボル表を小さくする
        format!("_x{name}")
    };
    Ok(FnSig {
        params,
        ret,
        symbol,
        is_extern,
        is_main,
    })
}
