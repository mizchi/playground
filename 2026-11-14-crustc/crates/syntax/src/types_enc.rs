//! 型の構文 (TySyn) と要約 (Summary) の直列化 (キャッシュ用)。front.rs から分けたもの: syn に依存しないので
//! driver2 (syn・Cranelift を使わないドライバ) も使え、crustc 自身でもコンパイルできる。

use crate::enc::{Enc, Rd};
use crate::types::TYS;
use crate::types::*;

impl Enc for TySyn {
    fn enc(&self, out: &mut Vec<u8>) {
        match self {
            TySyn::Prim(t) => out.push(TYS.iter().position(|x| x == t).unwrap() as u8),
            TySyn::Ref(m, i) => {
                out.push(20 + *m as u8);
                (**i).enc(out);
            }
            TySyn::Named(n) => {
                out.push(22);
                n.enc(out);
            }
            TySyn::App(n, a) => {
                out.push(23);
                n.enc(out);
                a.enc(out);
            }
            TySyn::Tuple(a) => {
                out.push(24);
                a.enc(out);
            }
            TySyn::Array(i, ArrLen::Lit(n)) => {
                out.push(25);
                (**i).enc(out);
                n.enc(out);
            }
            TySyn::Array(i, ArrLen::Const(c)) => {
                out.push(26);
                (**i).enc(out);
                c.enc(out);
            }
            TySyn::Slice(i) => {
                out.push(27);
                (**i).enc(out);
            }
            TySyn::Str => out.push(28),
            TySyn::Ptr(m, i) => {
                out.push(29 + *m as u8);
                (**i).enc(out);
            }
            TySyn::Dyn(n) => {
                out.push(31);
                n.enc(out);
            }
            TySyn::FnPtr(v) => {
                out.push(32);
                v.enc(out);
            }
            TySyn::Assoc(b, n) => {
                out.push(33);
                (**b).enc(out);
                n.enc(out);
            }
            TySyn::Infer => out.push(34),
        }
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some(match r.u8()? {
            c @ 0..18 => TySyn::Prim(TYS[c as usize]),
            c @ (20 | 21) => TySyn::Ref(c == 21, Box::new(TySyn::dec(r)?)),
            22 => TySyn::Named(String::dec(r)?),
            23 => TySyn::App(String::dec(r)?, Vec::dec(r)?),
            24 => TySyn::Tuple(Vec::dec(r)?),
            25 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Lit(u64::dec(r)?)),
            26 => TySyn::Array(Box::new(TySyn::dec(r)?), ArrLen::Const(String::dec(r)?)),
            27 => TySyn::Slice(Box::new(TySyn::dec(r)?)),
            28 => TySyn::Str,
            c @ (29 | 30) => TySyn::Ptr(c == 30, Box::new(TySyn::dec(r)?)),
            31 => TySyn::Dyn(String::dec(r)?),
            32 => TySyn::FnPtr(Vec::dec(r)?),
            33 => TySyn::Assoc(Box::new(TySyn::dec(r)?), String::dec(r)?),
            34 => TySyn::Infer,
            _ => return None,
        })
    }
}

impl Enc for VariantKind {
    fn enc(&self, out: &mut Vec<u8>) {
        out.push(*self as u8)
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        [VariantKind::Unit, VariantKind::Tuple, VariantKind::Struct]
            .get(r.u8()? as usize)
            .copied()
    }
    fn is_empty_val(&self) -> bool {
        *self == VariantKind::Unit
    }
}

crate::enc_struct!(VariantSyn {
    name,
    kind,
    fields,
    discr
});
crate::enc_struct!(SigSyn {
    params,
    ret,
    symbol,
    is_extern,
    is_main,
    impl_of,
    has_self,
    generics,
    trait_of,
    has_body,
    impl_trait,
    self_ty,
    export,
    panic_handler,
});
crate::enc_struct!(Summary {
    fns,
    structs,
    impl_self,
    enums,
    trait_decl,
    impl_trait,
    derives,
    adt_generics,
    impl_generics,
    impl_self_ty,
    consts,
    statics,
    closures,
    assocs,
    aliases,
});
crate::enc_struct!(AssocSyn {
    name,
    self_ty,
    generics,
    value,
});
crate::enc_struct!(ClosureSyn {
    name,
    generics,
    fields,
    sig,
    by_move,
    moved,
    ret_ann,
    n_parent,
});


impl Enc for (String, TySyn, String, bool) {
    fn enc(&self, out: &mut Vec<u8>) {
        self.0.enc(out);
        self.1.enc(out);
        self.2.enc(out);
        self.3.enc(out);
    }
    fn dec(r: &mut Rd) -> Option<Self> {
        Some((
            String::dec(r)?,
            TySyn::dec(r)?,
            String::dec(r)?,
            bool::dec(r)?,
        ))
    }
}

/// キャッシュにインスタンスの型引数を書くため
pub fn put_ty(out: &mut Vec<u8>, t: &TySyn) {
    t.enc(out)
}

/// d[*pos..] から TySyn を 1 つ読む
pub fn read_ty(d: &[u8], pos: &mut usize) -> Option<TySyn> {
    let mut r = Rd { d, pos: *pos };
    let t = TySyn::dec(&mut r);
    *pos = r.pos;
    t
}

impl Summary {
    pub fn encode(&self, out: &mut Vec<u8>) {
        self.enc(out)
    }

    pub fn decode(d: &[u8]) -> Option<Summary> {
        Summary::dec(&mut Rd::new(d))
    }
}

