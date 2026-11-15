//! 関数 1 個分の型推論 (union-find による単一化)。
//!
//! `let mut i = 0; while i < n { .. }` や `let mut i = 0; bump(&mut i)` のように、
//! 注釈のない let の型を後続の使用箇所から決めるためのパス。型エラーの報告はここでは
//! 行わず (矛盾は無視)、codegen 側の検査に任せる。結果は「注釈なし let → 型」の表。

use syn::{BinOp, Expr, Lit, Pat, Stmt, UnOp};

use crate::front::{FnSig, Program, Ty, TyEnv, intern};

/// 注釈なし let と、値を作る式の型 (ノードのアドレス → 型)
pub type LetTypes = crate::fxhash::FxMap<usize, Ty>;

/// 整数 / &str / 生ポインタの組み込みメソッド名 (同名の struct のメソッドがある場合でも、
/// 受け手が struct でなければこちら)
fn is_builtin_scalar_method(n: &str) -> bool {
    matches!(
        n,
        "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "wrapping_neg"
            | "checked_add"
            | "checked_sub"
            | "checked_mul"
            | "checked_div"
            | "checked_rem"
            | "checked_pow"
            | "wrapping_pow"
            | "saturating_pow"
            | "overflowing_pow"
            | "overflowing_add"
            | "overflowing_sub"
            | "overflowing_mul"
            | "rem_euclid"
            | "div_euclid"
            | "next_power_of_two"
            | "isqrt"
            | "try_into"
            | "checked_abs"
            | "checked_neg"
            | "leading_ones"
            | "trailing_ones"
            | "saturating_add"
            | "saturating_sub"
            | "abs"
            | "pow"
            | "count_ones"
            | "count_zeros"
            | "leading_zeros"
            | "trailing_zeros"
            | "is_power_of_two"
            | "rotate_left"
            | "rotate_right"
            | "swap_bytes"
            | "signum"
            | "is_positive"
            | "is_negative"
            | "abs_diff"
            | "is_ascii_digit"
            | "is_ascii_alphabetic"
            | "is_ascii_uppercase"
            | "is_ascii_lowercase"
            | "to_ascii_uppercase"
            | "to_ascii_lowercase"
            | "is_ascii_whitespace"
            | "is_ascii_alphanumeric"
            | "is_ascii_hexdigit"
            | "is_ascii_punctuation"
            | "is_ascii_graphic"
            | "is_ascii_control"
            | "is_ascii"
            | "to_le_bytes"
            | "to_be_bytes"
            | "to_ne_bytes"
            | "as_bytes"
            | "as_ptr"
            | "is_null"
            | "read_volatile"
            | "write_volatile"
            | "offset"
            | "wrapping_offset"
            | "read_unaligned"
            | "write_unaligned"
    )
}

/// `x.f` / `x.0` のフィールド名
fn member_name(m: &syn::Member) -> String {
    match m {
        syn::Member::Named(id) => id.to_string(),
        syn::Member::Unnamed(i) => i.index.to_string(),
    }
}

pub fn let_key(l: &syn::Local) -> usize {
    l as *const syn::Local as usize
}

#[derive(Clone, Copy)]
enum Shape {
    Known(Ty),
    /// 参照。中身は別の型変数
    Ref(bool, u32),
    /// ジェネリックな struct / enum (定義の id) に、型引数の型変数 (adt_args[start..start+len]) を入れたもの
    Adt(u32, u32, u32),
    /// `[T; N]` (要素の型変数, 長さ)
    Arr(u32, u32),
    /// `[T]`
    Slc(u32),
    /// `*const T` (false) / `*mut T` (true)
    Ptr(bool, u32),
    /// クロージャ: adt_args[start..start+n] が引数、その次が戻り値の型変数
    Clo(u32, u32),
    /// `Box<T>`
    Bx(u32),
}

struct Inf<'p> {
    prog: &'p Program,
    /// impl の中なら self 型
    self_sid: Option<u32>,
    env: &'p TyEnv,
    parent: Vec<u32>,
    shape: Vec<Option<Shape>>,
    /// 整数リテラル由来 (決まらなければ i32 にしてよい) か
    int_lit: Vec<bool>,
    /// 接尾辞のない浮動小数点リテラルの型変数 (決まらなければ f64)
    float_lit: Vec<bool>,
    scopes: Vec<Vec<(String, u32)>>,
    lets: Vec<(usize, u32)>,
    ret: u32,
    /// Shape::Adt の型引数の型変数
    adt_args: Vec<u32>,
    /// 呼び出しの引数を推論している深さと、本体の推論を後回しにしたクロージャ (式, 引数, 戻り値)
    defer: u32,
    pending: Vec<(*const syn::ExprClosure, Vec<u32>, u32)>,
    /// parse し直したマクロの引数。後回しにしたクロージャ (pending) が指すので推論の終わりまで持っておく
    keep: Vec<Vec<Expr>>,
    /// 囲んでいるループ / ラベル付きブロック: (ラベル, 値の型, ブロックか, break があったか)
    brk: Vec<(Option<String>, u32, bool, bool)>,
    /// 後で解く関連型 (基になる型の変数, 名前, 結果の変数)
    assoc_later: Vec<(u32, String, u32)>,
    /// 0 でなければ式の型を記録しない (一時的に parse したノードの推論中)
    no_record: u32,
    /// 0 でなければ式の型をソース上の位置で記録する (マクロの引数: codegen も同じ位置のノードを parse し直す)
    pos_record: u32,
    /// `it.collect()` の (受け手, 結果): 最後に結果が Vec<T> / HashMap<K, V> なら Item と単一化する
    collects: Vec<(u32, u32)>,
    /// 次の apply_recv に渡すメソッド呼び出しの turbofish
    turbofish: Option<*const syn::AngleBracketedGenericArguments>,
    /// クロージャの型を解くための文脈: (根の関数, closure_key → 前順の番号)
    clo_ctx: Option<(u32, crate::fxhash::FxMap<(usize, usize), u32>)>,
    /// (根の関数, 最初の番号, 本体): クロージャに出会ったら clo_ctx を作る
    clo_root: Option<(u32, u32, *const syn::Block)>,
    /// Shape::Clo の st → (クロージャの関数 id, キャプチャの型変数, 注釈のない引数の位置)
    clo_info: crate::fxhash::FxMap<u32, (u32, Vec<u32>, Vec<usize>)>,
    /// 後でもう一度見る関連型の境界 (呼び先, 型パラメータの型変数)
    late: Vec<(u32, Vec<Option<u32>>)>,
}

/// sig はこの関数 (インスタンスなら型引数を入れた後) のシグネチャ、env は `Self` と型パラメータ
pub fn infer(prog: &Program, f: &syn::ItemFn, sig: &FnSig, env: &TyEnv) -> LetTypes {
    infer_full(prog, f, sig, env, &[], Some(sig.ret)).0
}

/// 推論結果: (let と式の型, 戻り値型, 引数の型)
pub type Full = (LetTypes, Option<Ty>, Vec<Option<Ty>>);

/// extra は最初からスコープにある変数 (クロージャのキャプチャ)。ret が None なら戻り値型も推論して返す。
/// 型パラメータを含む引数の型は未知として扱い、推論した型を返す (注釈のないクロージャ引数)
pub fn infer_full(
    prog: &Program,
    f: &syn::ItemFn,
    sig: &FnSig,
    env: &TyEnv,
    extra: &[(String, Ty)],
    ret: Option<Ty>,
) -> Full {
    infer_ctx(prog, f, sig, env, extra, ret, None)
}

/// root = (根の関数, f の本体の最初のクロージャの前順の番号): 本体のクロージャの型 (struct) まで解く
pub fn infer_ctx(
    prog: &Program,
    f: &syn::ItemFn,
    sig: &FnSig,
    env: &TyEnv,
    extra: &[(String, Ty)],
    ret: Option<Ty>,
    root: Option<(u32, u32)>,
) -> Full {
    let clo_root = root.map(|(r, base)| (r, base, &*f.block as *const syn::Block));
    let mut s = Inf {
        prog,
        self_sid: env.self_sid,
        env,
        parent: vec![],
        shape: vec![],
        int_lit: vec![],
        float_lit: vec![],
        scopes: vec![vec![]],
        lets: vec![],
        ret: 0,
        adt_args: vec![],
        defer: 0,
        pending: vec![],
        keep: vec![],
        brk: vec![],
        assoc_later: vec![],
        no_record: 0,
        pos_record: 0,
        collects: vec![],
        turbofish: None,
        clo_ctx: None,
        clo_root,
        clo_info: Default::default(),
        late: vec![],
    };
    s.ret = match ret {
        // 一部だけ分かっている型 (`Result<?, u8>`): 型パラメータの所を型変数にする
        Some(t) if prog.has_param(t) => s.known_g(t, &mut vec![]),
        Some(t) => s.known(t),
        None => s.var(),
    };
    for (n, t) in extra {
        let v = s.known(*t);
        s.scopes[0].push((n.clone(), v));
    }
    let mut pvars = vec![];
    for (a, &t) in f.sig.inputs.iter().zip(&sig.params) {
        let name = match a {
            syn::FnArg::Receiver(_) => "self".to_string(),
            syn::FnArg::Typed(pt) => match &*pt.pat {
                Pat::Ident(pi) => pi.ident.to_string(),
                _ => {
                    pvars.push(s.var());
                    continue;
                }
            },
        };
        let v = if prog.has_param(t) { s.var() } else { s.known(t) };
        pvars.push(v);
        s.scopes[0].push((name, v));
    }
    let tail = s.block(&f.block);
    let ret = s.ret;
    s.unify(tail, ret);
    s.finish_late_bounds();
    s.finish_collects();
    s.finish_assocs();
    let lets = std::mem::take(&mut s.lets);
    let r = s.resolve(ret);
    let ps = pvars.into_iter().map(|v| s.resolve(v)).collect();
    (
        lets.into_iter()
            .filter_map(|(k, v)| Some((k, s.resolve(v)?)))
            .collect(),
        r,
        ps,
    )
}

impl Inf<'_> {
    fn var(&mut self) -> u32 {
        self.parent.push(self.parent.len() as u32);
        self.shape.push(None);
        self.int_lit.push(false);
        self.float_lit.push(false);
        self.parent.len() as u32 - 1
    }
    fn with(&mut self, sh: Shape) -> u32 {
        let v = self.var();
        self.shape[v as usize] = Some(sh);
        v
    }
    fn known(&mut self, t: Ty) -> u32 {
        match t {
            Ty::Ref(m, inner) => {
                let i = self.known(*inner);
                self.with(Shape::Ref(m, i))
            }
            Ty::Box(inner) => {
                let i = self.known(*inner);
                self.with(Shape::Bx(i))
            }
            // 関連型 (まだ決まっていない) は分からないものとして扱う
            Ty::Assoc(_) => self.var(),
            Ty::Array(e, n) => {
                let i = self.known(*e);
                self.with(Shape::Arr(i, n))
            }
            Ty::Ptr(m, e) => {
                let i = self.known(*e);
                self.with(Shape::Ptr(m, i))
            }
            Ty::Slice(e) => {
                let i = self.known(*e);
                self.with(Shape::Slc(i))
            }
            // 型パラメータを含む (まだ記号的な) インスタンスは Adt の形にする
            Ty::Struct(_) if self.prog.has_param(t) => {
                let mut pv = vec![];
                self.known_g(t, &mut pv)
            }
            t => self.with(Shape::Known(t)),
        }
    }
    fn adt(&mut self, base: u32, vars: Vec<u32>) -> u32 {
        let start = self.adt_args.len() as u32;
        let len = vars.len() as u32;
        self.adt_args.extend(vars);
        self.with(Shape::Adt(base, start, len))
    }
    fn adt_vars_of(&self, start: u32, len: u32) -> Vec<u32> {
        self.adt_args[start as usize..(start + len) as usize].to_vec()
    }
    /// v をジェネリックな定義 base のインスタンスとして見たときの型引数の型変数
    /// (分からなければ v にその形を与える)。非ジェネリックなら空
    fn adt_view(&mut self, v: u32, base: u32) -> Vec<u32> {
        let n = self.prog.structs[base as usize].generics.len();
        if n == 0 {
            return vec![];
        }
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Adt(b, st, len)) if b == base => self.adt_vars_of(st, len),
            Some(Shape::Known(Ty::Struct(i))) if self.prog.base_of(i) == base => {
                let args = self.prog.structs[i as usize].args.clone();
                args.into_iter().map(|a| self.known(a)).collect()
            }
            None => {
                let vars: Vec<u32> = (0..n).map(|_| self.var()).collect();
                let a = self.adt(base, vars.clone());
                self.unify(r, a);
                vars
            }
            _ => (0..n).map(|_| self.var()).collect(),
        }
    }
    /// ジェネリックな定義 base のフィールド型 (Param を含む) を、型引数の型変数 vars で具体化した型変数
    fn field_var(&mut self, t: Ty, vars: &[u32]) -> u32 {
        let mut pv: Vec<Option<u32>> = vars.iter().map(|&v| Some(v)).collect();
        self.known_g(t, &mut pv)
    }
    fn find(&mut self, mut v: u32) -> u32 {
        while self.parent[v as usize] != v {
            let p = self.parent[v as usize];
            self.parent[v as usize] = self.parent[p as usize];
            v = p;
        }
        v
    }
    fn link(&mut self, from: u32, to: u32) {
        self.parent[from as usize] = to;
        self.int_lit[to as usize] |= self.int_lit[from as usize];
        self.float_lit[to as usize] |= self.float_lit[from as usize];
    }
    /// `==` の両辺: Vec と配列 (`PartialEq<[U; N]> for Vec<T>`) なら要素の型どうし、それ以外は普通に単一化する
    fn seq_unify(&mut self, a: u32, b: u32) {
        let (fa, fb) = (self.find(a), self.find(b));
        let elem = |s: &mut Self, v: u32| -> Option<u32> {
            match s.shape[v as usize] {
                Some(Shape::Arr(x, _)) | Some(Shape::Slc(x)) => Some(x),
                Some(Shape::Adt(vb, st, 1)) if s.is_vec(vb) => Some(s.adt_args[st as usize]),
                Some(Shape::Known(Ty::Struct(sid))) if s.is_vec(s.prog.base_of(sid)) => {
                    s.prog.structs[sid as usize].args.first().map(|&t| s.known(t))
                }
                Some(Shape::Known(Ty::Array(e, _) | Ty::Slice(e))) => Some(s.known(*e)),
                _ => None,
            }
        };
        let same_kind = std::mem::discriminant(&self.shape[fa as usize]) == std::mem::discriminant(&self.shape[fb as usize]);
        match (elem(self, fa), elem(self, fb)) {
            (Some(x), Some(y)) if !same_kind || matches!(self.shape[fa as usize], Some(Shape::Adt(..) | Shape::Known(Ty::Struct(_)))) => {
                // (要素どうしも型が違ってよい (`Vec<String> == [&str]`): 片方が分からないときだけ決める)
                self.elem_hint(x, y)
            }
            _ => self.unify(a, b),
        }
    }

    /// 要素の型: 片方がまだ決まっていなければ単一化する (数のリテラルを相手の型にする)
    fn elem_hint(&mut self, x: u32, y: u32) {
        let (fx, fy) = (self.find(x), self.find(y));
        // (数のリテラルだけ: 要素の型がまだ決まっていない Vec と文字列の配列は単一化しない)
        let lit = |s: &Self, v: u32| s.shape[v as usize].is_none() && s.int_lit[v as usize];
        if lit(self, fx) || lit(self, fy) {
            self.unify(x, y);
        }
    }

    fn unify(&mut self, a: u32, b: u32) {
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return;
        }
        match (self.shape[a as usize], self.shape[b as usize]) {
            (Some(Shape::Known(Ty::Never)), _) | (_, Some(Shape::Known(Ty::Never))) => {}
            (None, _) => self.link(a, b),
            (_, None) => self.link(b, a),
            (Some(Shape::Ref(_, x)), Some(Shape::Ref(_, y))) => {
                // `&T` と `&dyn Trait`: 強制変換なので中身は単一化せず、dyn の方を代表にする
                let is_dyn = |s: &mut Self, v: u32| {
                    let r = s.find(v);
                    matches!(s.shape[r as usize], Some(Shape::Known(Ty::Dyn(_))))
                };
                let is_clo = |s: &mut Self, v: u32| {
                    let r = s.find(v);
                    matches!(s.shape[r as usize], Some(Shape::Clo(..)))
                };
                match (is_dyn(self, x), is_dyn(self, y)) {
                    (true, false) => {
                        self.link(b, a);
                        // `&|x| ..` を `&dyn Fn(A) -> B` に: クロージャの引数・戻り値は決まる
                        if is_clo(self, y) {
                            self.unify(x, y);
                        }
                    }
                    (false, true) => {
                        self.link(a, b);
                        if is_clo(self, x) {
                            self.unify(x, y);
                        }
                    }
                    _ => {
                        // (`&Vec<T>` / `&[T; N]` と `&[T]` は強制変換で渡すだけ: 参照どうしは同じ型にしない。
                        //  つなぐとクロージャがキャプチャした `v: &Vec<T>` が `&[T]` になってしまう)
                        let slc = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            matches!(s.shape[r as usize], Some(Shape::Slc(_)))
                        };
                        let seqish = |s: &mut Self, v: u32| {
                            let r = s.find(v);
                            match s.shape[r as usize] {
                                Some(Shape::Arr(..)) => true,
                                Some(Shape::Adt(b, _, 1)) => s.is_vec(b),
                                Some(Shape::Known(Ty::Struct(sid))) => s.is_vec(s.prog.base_of(sid)),
                                _ => false,
                            }
                        };
                        let coerce = (slc(self, x) && seqish(self, y)) || (slc(self, y) && seqish(self, x));
                        if !coerce {
                            self.link(a, b);
                        }
                        self.unify(x, y);
                    }
                }
            }
            (Some(Shape::Known(x)), Some(Shape::Known(y))) if x == y => self.link(a, b),
            (Some(Shape::Arr(x, n)), Some(Shape::Arr(y, m))) if n == m => {
                self.link(a, b);
                self.unify(x, y);
            }
            (Some(Shape::Slc(x)), Some(Shape::Slc(y)))
            | (Some(Shape::Bx(x)), Some(Shape::Bx(y)))
            | (Some(Shape::Ptr(_, x)), Some(Shape::Ptr(_, y))) => {
                self.link(a, b);
                self.unify(x, y);
            }
            (Some(Shape::Adt(x, s1, l1)), Some(Shape::Adt(y, s2, l2))) if x == y => {
                self.link(a, b);
                for (p, q) in self
                    .adt_vars_of(s1, l1)
                    .into_iter()
                    .zip(self.adt_vars_of(s2, l2))
                {
                    self.unify(p, q);
                }
            }
            (Some(Shape::Adt(x, st, l)), Some(Shape::Known(Ty::Struct(i))))
            | (Some(Shape::Known(Ty::Struct(i))), Some(Shape::Adt(x, st, l)))
                if self.prog.base_of(i) == x =>
            {
                // 具体型の方を代表にする
                let (adt_root, known_root) =
                    if matches!(self.shape[a as usize], Some(Shape::Adt(..))) {
                        (a, b)
                    } else {
                        (b, a)
                    };
                self.link(adt_root, known_root);
                let args = self.prog.structs[i as usize].args.clone();
                for (p, t) in self.adt_vars_of(st, l).into_iter().zip(args) {
                    let k = self.known(t);
                    self.unify(p, k);
                }
            }
            // `&Vec<T>` を `&[T]` に渡す (強制変換): 要素の型だけ単一化する
            (Some(Shape::Slc(x)), Some(Shape::Adt(vb, st, 1)))
            | (Some(Shape::Adt(vb, st, 1)), Some(Shape::Slc(x)))
                if self.is_vec(vb) =>
            {
                let y = self.adt_args[st as usize];
                self.unify(x, y)
            }
            (Some(Shape::Slc(x)), Some(Shape::Known(Ty::Struct(sid))))
            | (Some(Shape::Known(Ty::Struct(sid))), Some(Shape::Slc(x)))
                if self.is_vec(self.prog.base_of(sid)) =>
            {
                if let Some(&a) = self.prog.structs[sid as usize].args.first() {
                    let k = self.known(a);
                    self.unify(x, k);
                }
            }
            // `&[T; N]` を `&[T]` に渡す (強制変換): 要素の型だけ単一化する
            (Some(Shape::Slc(x)), Some(Shape::Arr(y, _)))
            | (Some(Shape::Arr(y, _)), Some(Shape::Slc(x))) => self.unify(x, y),
            (Some(Shape::Clo(s1, n1)), Some(Shape::Clo(s2, n2))) if n1 == n2 => {
                self.link(a, b);
                for i in 0..=n1 {
                    let (p, q) = (self.adt_args[(s1 + i) as usize], self.adt_args[(s2 + i) as usize]);
                    self.unify(p, q);
                }
            }
            (Some(Shape::Clo(st, n)), Some(Shape::Known(t)))
            | (Some(Shape::Known(t)), Some(Shape::Clo(st, n))) => {
                let (ca, ka) = if matches!(self.shape[a as usize], Some(Shape::Clo(..))) {
                    (a, b)
                } else {
                    (b, a)
                };
                if let Some((ps, r)) = self.closure_sig(t) {
                    if ps.len() == n as usize {
                        // `dyn Fn` とは同じ型ではない (強制変換されるだけ) ので要素だけ単一化する
                        if !matches!(t, Ty::Dyn(_)) {
                            self.link(ca, ka);
                        }
                        for (i, t) in ps.into_iter().chain([r]).enumerate() {
                            let k = self.known(t);
                            let p = self.adt_args[st as usize + i];
                            self.unify(p, k);
                        }
                    }
                }
            }
            _ => {} // 矛盾は codegen で報告
        }
    }
    /// 具体型のクロージャ (struct のインスタンス / `&dyn Fn`) の引数と戻り値の型
    fn closure_sig(&self, t: Ty) -> Option<(Vec<Ty>, Ty)> {
        match t {
            Ty::Fn(f) => Some((f.params.clone(), f.ret)),
            Ty::Struct(sid) => {
                let cid = *self.prog.closure_of_struct.get(&self.prog.base_of(sid))?;
                let args = &self.prog.structs[sid as usize].args;
                let s0 = &self.prog.sigs[cid as usize];
                Some((
                    s0.params[1..].iter().map(|&p| self.prog.subst(p, args)).collect(),
                    self.prog.subst(s0.ret, args),
                ))
            }
            Ty::Dyn(s) => {
                let (_, ps, r) = crate::front::parse_fn_bound(s)?;
                let ps = ps
                    .iter()
                    .map(|t| self.prog.resolve_env(t, self.env).ok())
                    .collect::<Option<Vec<Ty>>>()?;
                Some((ps, self.prog.resolve_env(&r, self.env).ok()?))
            }
            _ => None,
        }
    }
    /// 呼び出せる値の (引数, 戻り値) の型変数。参照は剥がす
    fn callable(&mut self, f: u32, n: usize) -> Option<(Vec<u32>, u32)> {
        let mut f = f;
        loop {
            let r = self.find(f);
            match self.shape[r as usize] {
                Some(Shape::Ref(_, i)) | Some(Shape::Bx(i)) => f = i,
                Some(Shape::Clo(st, k)) if k as usize == n => {
                    let vs = self.adt_args[st as usize..=(st + k) as usize].to_vec();
                    return Some((vs[..n].to_vec(), vs[n]));
                }
                Some(Shape::Known(Ty::Ref(_, t))) => {
                    let (ps, r) = self.closure_sig(*t)?;
                    let ps = ps.into_iter().map(|t| self.known(t)).collect();
                    return Some((ps, self.known(r)));
                }
                Some(Shape::Known(t)) => {
                    let (ps, r) = self.closure_sig(t)?;
                    let ps = ps.into_iter().map(|t| self.known(t)).collect();
                    return Some((ps, self.known(r)));
                }
                _ => return None,
            }
        }
    }
    /// 型変数 v の関連型 name の型変数 (`Range<?T>` の Item → ?T、`Filter<I, P>` の Item → I の Item)
    fn assoc_var(&mut self, v: u32, name: &str) -> Option<u32> {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Known(t)) => {
                let a = self.prog.assoc_of(t, name)?;
                Some(self.known(a))
            }
            Some(Shape::Adt(b, st, l)) => {
                let prog = self.prog;
                let def = prog.assocs.iter().find(|a| {
                    a.name == name
                        && matches!(a.self_ty, Ty::Struct(sid) if prog.base_of(sid) == b)
                })?;
                let Ty::Struct(ssid) = def.self_ty else { return None };
                let sargs = prog.structs[ssid as usize].args.clone();
                let vars = self.adt_vars_of(st, l);
                let mut pv: Vec<Option<u32>> = vec![None; def.ngen];
                for (a, &vv) in sargs.iter().zip(&vars) {
                    if let Ty::Param(i) = a {
                        if let Some(slot) = pv.get_mut(*i as usize) {
                            *slot = Some(vv);
                        }
                    }
                }
                let value = def.value;
                Some(self.known_g(value, &mut pv))
            }
            // (参照の型への impl の関連型: `impl IntoIterator for &Coll` の IntoIter)
            Some(Shape::Ref(..)) => {
                let t = self.resolve(v)?;
                let a = self.prog.assoc_of(t, name)?;
                Some(self.known(a))
            }
            _ => None,
        }
    }

    /// 型パラメータの関連型の制約 (`Self: Iterator<Item = S>`) で単一化する
    fn unify_assoc_bounds(&mut self, generics: &[(String, Vec<String>)], pv: &mut Vec<Option<u32>>) {
        let names: Vec<&str> = generics.iter().map(|g| g.0.as_str()).collect();
        for (g, (_, bounds)) in generics.iter().enumerate() {
            let Some(v) = pv.get(g).copied().flatten() else { continue };
            for b in bounds {
                if !b.contains("Item") || !b.contains('=') {
                    continue;
                }
                let Ok(tb) = syn::parse_str::<syn::TraitBound>(b) else { continue };
                let Some(syn::PathArguments::AngleBracketed(ab)) =
                    tb.path.segments.last().map(|s| &s.arguments)
                else {
                    continue;
                };
                for a in &ab.args {
                    let syn::GenericArgument::AssocType(at) = a else { continue };
                    let Ok(ts) = crate::front::parse_tysyn(&at.ty, None) else { continue };
                    let Ok(want) = self.prog.resolve_g(&ts, &names) else { continue };
                    let w = self.known_g(want, pv);
                    if let Some(actual) = self.assoc_var(v, &at.ident.to_string()) {
                        self.unify(w, actual);
                    }
                }
            }
        }
    }

    /// 型パラメータに `Fn(A) -> B` の境界があり、渡したのがクロージャなら A / B と単一化する
    fn unify_fn_bounds(&mut self, generics: &[(String, Vec<String>)], pv: &mut Vec<Option<u32>>) {
        for (g, (_, bounds)) in generics.iter().enumerate() {
            if !bounds.iter().any(|b| b.starts_with("Fn")) {
                continue;
            }
            let Some(v) = pv.get(g).copied().flatten() else { continue };
            let Some((_, ps, r)) = bounds.iter().find_map(|b| crate::front::parse_fn_bound(b)) else {
                continue;
            };
            let names: Vec<&str> = generics.iter().map(|g| g.0.as_str()).collect();
            let Some((cps, cr)) = self.callable(v, ps.len()) else { continue };
            for (t, c) in ps.iter().zip(cps) {
                if let Ok(t) = self.prog.resolve_g(t, &names) {
                    let k = self.known_g(t, pv);
                    self.unify(c, k);
                }
            }
            if let Ok(t) = self.prog.resolve_g(&r, &names) {
                let k = self.known_g(t, pv);
                self.unify(cr, k);
            }
        }
    }
    fn resolve(&mut self, v: u32) -> Option<Ty> {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Known(t)) => Some(t),
            Some(Shape::Ref(m, i)) => Some(Ty::Ref(m, intern(self.resolve(i)?))),
            Some(Shape::Bx(i)) => Some(Ty::Box(intern(self.resolve(i)?))),
            Some(Shape::Arr(e, n)) => Some(Ty::Array(intern(self.resolve(e)?), n)),
            Some(Shape::Slc(e)) => Some(Ty::Slice(intern(self.resolve(e)?))),
            Some(Shape::Ptr(m, e)) => Some(Ty::Ptr(m, intern(self.resolve(e)?))),
            Some(Shape::Adt(b, st, l)) => {
                let mut args = vec![];
                for v in self.adt_vars_of(st, l) {
                    args.push(self.resolve(v)?);
                }
                self.prog.instantiate(b, args).ok().map(Ty::Struct)
            }
            Some(Shape::Clo(st, n)) => {
                // クロージャの struct: 親の型引数 + キャプチャ + 注釈のない引数 + 戻り値
                let (fid, caps, free) = self.clo_info.get(&st)?.clone();
                let cd = self.prog.closures.get(&fid)?;
                let (n_parent, by_move, sid) = (cd.n_parent, cd.by_move, cd.sid);
                let mut args = vec![];
                for i in 0..n_parent {
                    args.push(self.env.params.get(i)?.1);
                }
                for cv in caps {
                    let t = self.resolve(cv)?;
                    args.push(if by_move { t } else { Ty::Ptr(true, intern(t)) });
                }
                for i in free {
                    let pv = self.adt_args[st as usize + i];
                    args.push(self.resolve(pv)?);
                }
                let rv = self.adt_args[(st + n) as usize];
                args.push(self.resolve(rv)?);
                self.prog.instantiate(sid, args).ok().map(Ty::Struct)
            }
            None if self.float_lit[r as usize] => Some(Ty::F64),
            None if self.int_lit[r as usize] => Some(Ty::I32),
            None => None,
        }
    }
    fn deref(&mut self, v: u32) -> u32 {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Ref(_, i)) | Some(Shape::Ptr(_, i)) | Some(Shape::Bx(i)) => i,
            _ => self.var(),
        }
    }
    /// 自動 deref しながらフィールドの型を引く
    fn field(&mut self, base: u32, name: &str) -> u32 {
        let v = self.deref_expr(base);
        let r = self.find(v);
        let sid = match self.shape[r as usize] {
            Some(Shape::Known(Ty::Struct(sid))) => sid,
            Some(Shape::Adt(b, _, _)) => b,
            _ => return self.var(),
        };
        let b = self.prog.base_of(sid);
        let vars = self.adt_view(v, b);
        match self.prog.field(b, name) {
            Some((ft, _)) => self.field_var(ft, &vars),
            None => self.var(),
        }
    }

    /// struct (vi = None) / バリアント (vi = Some) の値を作る式: 実引数をフィールド型と単一化する
    fn adt_ctor(&mut self, sid: u32, vi: Option<usize>, args: Vec<(String, u32)>) -> u32 {
        let base = self.prog.base_of(sid);
        let n = self.prog.structs[base as usize].generics.len();
        let vars: Vec<u32> = (0..n).map(|_| self.var()).collect();
        let fields: Vec<(String, Ty)> = {
            let sd = &self.prog.structs[base as usize];
            let f = match vi {
                Some(vi) => &sd.variants[vi].fields,
                None => &sd.fields,
            };
            f.iter().map(|x| (x.0.clone(), x.1)).collect()
        };
        for (name, a) in args {
            if let Some((_, ft)) = fields.iter().find(|f| f.0 == name) {
                let k = self.field_var(*ft, &vars);
                self.unify(a, k);
            }
        }
        if n == 0 {
            self.known(Ty::Struct(base))
        } else {
            self.adt(base, vars)
        }
    }

    /// 式が (参照越しの) 配列 / スライスなら Some。受け手の型を見るためだけに先に推論する
    fn expr_peek_elem(&mut self, e: &Expr) -> Option<u32> {
        // 受け手は後でもう一度 expr() されるが、同じ式なので型変数は単一化されて一致する
        let v = self.expr(e);
        // (Vec は prelude のメソッドを持つので、配列・スライスの組み込みメソッドには回さない)
        let d = self.deref_expr(v);
        let r = self.find(d);
        if !matches!(self.shape[r as usize], Some(Shape::Arr(..)) | Some(Shape::Slc(_))) {
            return None;
        }
        self.elem_of(v)
    }

    /// 配列 / スライス (参照越しも) の要素の型変数
    fn elem_of(&mut self, v: u32) -> Option<u32> {
        let v = self.deref_expr(v);
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Arr(e, _)) | Some(Shape::Slc(e)) => Some(e),
            // Vec<T> も添字・for で T を取り出せる
            Some(Shape::Known(Ty::Struct(sid))) if self.is_vec(self.prog.base_of(sid)) => {
                let a = *self.prog.structs[sid as usize].args.first()?;
                Some(self.known(a))
            }
            Some(Shape::Adt(b, st, l)) if self.is_vec(b) && l == 1 => {
                Some(self.adt_args[st as usize])
            }
            _ => None,
        }
    }

    fn is_vec(&self, base: u32) -> bool {
        self.prog.struct_index.get("Vec") == Some(&base)
    }

    /// Option<t> の型変数
    fn option_of(&mut self, t: u32) -> u32 {
        match self.prog.struct_index.get("Option") {
            Some(&ob) => self.adt(ob, vec![t]),
            None => self.var(),
        }
    }

    /// パターンのフィールド: v (auto-deref 済み) を base のインスタンスとして見て、フィールド型の型変数を返す
    fn pat_fields(&mut self, v: u32, sid: u32, vi: Option<usize>) -> Vec<(String, u32)> {
        let base = self.prog.base_of(sid);
        let vars = self.adt_view(v, base);
        if vars.is_empty() {
            let k = self.known(Ty::Struct(base));
            self.unify(v, k);
        }
        let fields: Vec<(String, Ty)> = {
            let sd = &self.prog.structs[base as usize];
            let f = match vi {
                Some(vi) => &sd.variants[vi].fields,
                None => &sd.fields,
            };
            f.iter().map(|x| (x.0.clone(), x.1)).collect()
        };
        fields
            .into_iter()
            .map(|(n, t)| (n, self.field_var(t, &vars)))
            .collect()
    }
    /// 呼び出し: 実引数を仮引数型と単一化して戻り値型を返す。skip はレシーバの分
    /// 型パラメータを含む型を、呼び出しごとの新しい型変数 (pv) で置き換えて作る
    fn known_g(&mut self, t: Ty, pv: &mut Vec<Option<u32>>) -> u32 {
        match t {
            Ty::Param(i) => match pv.get(i as usize).copied().flatten() {
                Some(v) => v,
                None => {
                    let v = self.var();
                    if pv.len() <= i as usize {
                        pv.resize(i as usize + 1, None);
                    }
                    pv[i as usize] = Some(v);
                    v
                }
            },
            Ty::Ref(m, inner) => {
                let i = self.known_g(*inner, pv);
                self.with(Shape::Ref(m, i))
            }
            Ty::Box(inner) => {
                let i = self.known_g(*inner, pv);
                self.with(Shape::Bx(i))
            }
            Ty::Array(e, n) => {
                let i = self.known_g(*e, pv);
                self.with(Shape::Arr(i, n))
            }
            Ty::Ptr(m, e) => {
                let i = self.known_g(*e, pv);
                self.with(Shape::Ptr(m, i))
            }
            Ty::Slice(e) => {
                let i = self.known_g(*e, pv);
                self.with(Shape::Slc(i))
            }
            Ty::Struct(sid) if self.prog.has_param(t) => {
                let sd = &self.prog.structs[sid as usize];
                let (base, args) = (sd.base.unwrap(), sd.args.clone());
                let vars: Vec<u32> = args.into_iter().map(|a| self.known_g(a, pv)).collect();
                self.adt(base, vars)
            }
            Ty::Assoc(a) => {
                let b = self.known_g(a.base, pv);
                let name = a.name.clone();
                match self.assoc_var(b, &name) {
                    Some(v) => v,
                    // (`T::Err` の T がまだ分からない: 最後に解く)
                    None => {
                        let v = self.var();
                        self.assoc_later.push((b, name, v));
                        v
                    }
                }
            }
            t => self.with(Shape::Known(t)),
        }
    }

    /// 呼び出し: 実引数を仮引数型と単一化して戻り値型を返す。skip はレシーバの分、
    /// self_ty はレシーバの型 (trait のデフォルトメソッドの Self を決める)
    fn apply(&mut self, callee: Option<u32>, args: Vec<u32>, skip: usize) -> u32 {
        self.apply_self(callee, args, skip, None)
    }

    /// メソッド呼び出し: recv はレシーバ (auto-deref 済み) の型変数。`Self` (impl の self 型 / trait の Self) と単一化する
    /// prelude の helper `f(recv, args..)` の戻り値 (turbofish は型引数の前から)
    fn apply_helper(&mut self, id: u32, mc: &syn::ExprMethodCall, args: Vec<u32>) -> u32 {
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, ng) = (sig.params[1.min(sig.params.len())..].to_vec(), sig.ret, sig.generics.len());
        let mut pv: Vec<Option<u32>> = vec![None; ng];
        if let Some(tf) = &mc.turbofish {
            for (i, a) in tf.args.iter().enumerate().take(ng) {
                if let syn::GenericArgument::Type(t) = a {
                    if let Ok(t) = self.prog.parse_ty(t, self.env) {
                        pv[i] = Some(self.known(t));
                    }
                }
            }
        }
        for (a, p) in args.into_iter().zip(params) {
            // (char を渡す &str の引数は推論では見ない)
            if p == Ty::Ref(false, intern(Ty::Str)) {
                continue;
            }
            let k = self.known_g(p, &mut pv);
            self.unify(a, k);
        }
        self.known_g(ret, &mut pv)
    }

    fn apply_recv(&mut self, callee: Option<u32>, args: Vec<u32>, recv: u32) -> u32 {
        let Some(id) = callee else { return self.var() };
        let sig = &self.prog.sigs[id as usize];
        let (params, ret, self_ty, is_trait) = (
            sig.params[1.min(sig.params.len())..].to_vec(),
            sig.ret,
            sig.self_ty,
            sig.trait_of.is_some(),
        );
        let mut pv: Vec<Option<u32>> = vec![None; sig.generics.len()];
        if is_trait {
            pv[0] = Some(recv);
        } else if let Some(st) = self_ty {
            let k = self.known_g(st, &mut pv);
            self.unify(k, recv);
        }
        for (a, p) in args.into_iter().zip(params) {
            let k = self.known_g(p, &mut pv);
            self.unify(a, k);
        }
        // turbofish (`collect::<Vec<_>>()`): 後ろの型パラメータ (メソッド自身のもの) から
        if let Some(tf) = self.turbofish.take() {
            // SAFETY: tf は infer() の間生きている関数の AST の中を指す
            let tf = unsafe { &*tf };
            let tys: Vec<&syn::Type> = tf
                .args
                .iter()
                .filter_map(|a| if let syn::GenericArgument::Type(t) = a { Some(t) } else { None })
                .collect();
            let ng = pv.len();
            if tys.len() <= ng {
                for (k, t) in tys.into_iter().enumerate() {
                    let v = self.syn_ty_var(t);
                    let i = ng - tf.args.len() + k;
                    match pv[i] {
                        Some(old) => self.unify(old, v),
                        None => pv[i] = Some(v),
                    }
                }
            }
        }
        let prog = self.prog;
        self.unify_fn_bounds(&prog.sigs[id as usize].generics, &mut pv);
        self.unify_assoc_bounds(&prog.sigs[id as usize].generics, &mut pv);
        let r = self.known_g(ret, &mut pv);
        self.late_bounds(id, pv);
        r
    }

    /// 関連型の境界 (`U: __Flat<Item = B>`) は、クロージャの本体を推論した後でもう一度見る
    fn late_bounds(&mut self, id: u32, pv: Vec<Option<u32>>) {
        let gs = &self.prog.sigs[id as usize].generics;
        if gs.iter().any(|g| g.1.iter().any(|b| b.contains("Item") && b.contains('='))) {
            self.late.push((id, pv));
        }
    }

    /// 書式マクロの引数 (`println!("{}", (1..4).map(..))`) も推論する。parse し直したノードは捨てるので、
    /// 型はソース上の位置で記録する (codegen も同じ位置のノードを parse して引く)
    fn fmt_args(&mut self, m: &syn::Macro) {
        use syn::parse::Parser;
        let Ok(args) = syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated.parse2(m.tokens.clone()) else {
            return;
        };
        let xs: Vec<Expr> = args
            .into_iter()
            .filter(|a| !matches!(a, Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(_), .. })))
            .map(|a| match a {
                // 名前つき引数 `w = expr`
                Expr::Assign(asg) => *asg.right,
                a => a,
            })
            .collect();
        if xs.is_empty() {
            return;
        }
        self.no_record += 1;
        self.pos_record += 1;
        let ki = self.keep.len();
        self.keep.push(xs);
        for i in 0..self.keep[ki].len() {
            let x: *const Expr = &self.keep[ki][i];
            self.expr(unsafe { &*x });
        }
        self.pos_record -= 1;
        self.no_record -= 1;
    }

    /// 書式文字列の幅・精度に名前で使う変数 (`{:w$}` / `{:.p$}`) は usize
    fn fmt_width_vars(&mut self, m: &syn::Macro) {
        self.fmt_args(m);
        let Some(lit) = m.tokens.clone().into_iter().find_map(|t| match t {
            proc_macro2::TokenTree::Literal(l) => syn::parse_str::<syn::LitStr>(&l.to_string()).ok(),
            _ => None,
        }) else {
            return;
        };
        let s = lit.value();
        if !s.contains('$') {
            return;
        }
        let b = s.as_bytes();
        for (i, _) in s.match_indices('$') {
            let mut k = i;
            while k > 0 && (b[k - 1].is_ascii_alphanumeric() || b[k - 1] == b'_') {
                k -= 1;
            }
            let name = &s[k..i];
            if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
                continue;
            }
            if let Some(v) = self.lookup(name) {
                let u = self.known(Ty::Usize);
                self.unify(v, u);
            }
        }
    }

    /// 書かれた型の型変数 (`Vec<_>` の `_` は新しい型変数)
    fn syn_ty_var(&mut self, t: &syn::Type) -> u32 {
        if let Ok(k) = self.prog.parse_ty(t, self.env) {
            return self.known(k);
        }
        match t {
            syn::Type::Infer(_) => self.var(),
            syn::Type::Reference(r) => {
                let i = self.syn_ty_var(&r.elem);
                self.with(Shape::Ref(r.mutability.is_some(), i))
            }
            syn::Type::Path(p) if p.qself.is_none() => {
                let last = p.path.segments.last().unwrap();
                let base = self.prog.struct_index.get(&last.ident.to_string()).copied();
                match (base, &last.arguments) {
                    (Some(b), syn::PathArguments::AngleBracketed(ab)) => {
                        let args: Vec<u32> = ab
                            .args
                            .iter()
                            .filter_map(|a| if let syn::GenericArgument::Type(t) = a { Some(t) } else { None })
                            .map(|t| self.syn_ty_var(t))
                            .collect();
                        self.adt(b, args)
                    }
                    _ => self.var(),
                }
            }
            syn::Type::Tuple(tt) => {
                let args: Vec<u32> = tt.elems.iter().map(|t| self.syn_ty_var(t)).collect();
                let tb = self.prog.tuple_base(args.len());
                self.adt(tb, args)
            }
            _ => self.var(),
        }
    }

    fn apply_self(
        &mut self,
        callee: Option<u32>,
        args: Vec<u32>,
        skip: usize,
        self_ty: Option<Ty>,
    ) -> u32 {
        match callee {
            Some(id) => {
                let sig = &self.prog.sigs[id as usize];
                let (params, ret) = (sig.params[skip.min(sig.params.len())..].to_vec(), sig.ret);
                let mut pv: Vec<Option<u32>> = vec![None; sig.generics.len()];
                if let (Some(t), true) = (self_ty, sig.trait_of.is_some()) {
                    pv[0] = Some(self.known(t));
                }
                for (a, p) in args.into_iter().zip(params) {
                    let k = self.known_g(p, &mut pv);
                    self.unify(a, k);
                }
                let prog = self.prog;
                self.unify_fn_bounds(&prog.sigs[id as usize].generics, &mut pv);
                self.unify_assoc_bounds(&prog.sigs[id as usize].generics, &mut pv);
                let r = self.known_g(ret, &mut pv);
                self.late_bounds(id, pv);
                r
            }
            None => self.var(),
        }
    }

    fn lit_var(&mut self, l: &Lit) -> u32 {
        match l {
            Lit::Int(i) if matches!(i.suffix(), "f32" | "f64") => {
                self.known(if i.suffix() == "f32" { Ty::F32 } else { Ty::F64 })
            }
            Lit::Float(f) => match f.suffix() {
                "f32" => self.known(Ty::F32),
                "f64" => self.known(Ty::F64),
                _ => {
                    let v = self.var();
                    self.float_lit[v as usize] = true;
                    v
                }
            },
            Lit::Int(i) => match Ty::from_suffix(i.suffix()) {
                Some(t) => self.known(t),
                None => {
                    let v = self.var();
                    self.int_lit[v as usize] = true;
                    v
                }
            },
            Lit::Bool(_) => self.known(Ty::Bool),
            Lit::Byte(_) => self.known(Ty::U8),
            Lit::Char(_) => self.known(Ty::Char),
            Lit::Str(_) => self.known(Ty::Ref(false, intern(Ty::Str))),
            Lit::ByteStr(b) => self.known(Ty::Ref(
                false,
                intern(Ty::Array(intern(Ty::U8), b.value().len() as u32)),
            )),
            _ => self.var(),
        }
    }

    /// 参照型の変数を辿って中身へ。参照を通ったら束縛モードを Ref にする
    /// FromIterator の代わり: collect の結果が Vec<T> なら Item = T、HashMap<K, V> なら Item = (K, V)
    /// `extend` の引数の要素の型 (Vec / BinaryHeap は要素、参照・配列は要素の参照、イテレータは Item)
    fn extend_item(&self, t: Ty) -> Option<Ty> {
        let elem_of = |t: Ty| -> Option<Ty> {
            match t {
                Ty::Array(e, _) | Ty::Slice(e) => Some(*e),
                Ty::Struct(s)
                    if matches!(
                        self.prog.structs[self.prog.base_of(s) as usize].name.as_str(),
                        "Vec" | "BinaryHeap"
                    ) =>
                {
                    self.prog.structs[s as usize].args.first().copied()
                }
                _ => None,
            }
        };
        match t {
            Ty::Ref(_, i) => elem_of(*i).map(|e| Ty::Ref(false, intern(e))),
            Ty::Array(e, _) => Some(Ty::Ref(false, e)),
            t => elem_of(t).or_else(|| self.prog.assoc_of(t, "Item")),
        }
    }

    fn finish_late_bounds(&mut self) {
        let ls = std::mem::take(&mut self.late);
        for (id, mut pv) in ls {
            let prog = self.prog;
            self.unify_assoc_bounds(&prog.sigs[id as usize].generics, &mut pv);
        }
    }

    fn finish_assocs(&mut self) {
        for _ in 0..2 {
            let ls = std::mem::take(&mut self.assoc_later);
            for (b, name, v) in ls {
                match self.assoc_var(b, &name) {
                    Some(a) => self.unify(a, v),
                    None => self.assoc_later.push((b, name, v)),
                }
            }
        }
    }

    fn finish_collects(&mut self) {
        let cs = std::mem::take(&mut self.collects);
        for (recv, r) in cs {
            let Some(item) = self.assoc_var(recv, "Item") else { continue };
            let root = self.find(r);
            let (b, vars) = match self.shape[root as usize] {
                Some(Shape::Adt(b, st, l)) => (b, self.adt_vars_of(st, l)),
                Some(Shape::Known(Ty::Struct(sid))) => {
                    let args = self.prog.structs[sid as usize].args.clone();
                    (self.prog.base_of(sid), args.into_iter().map(|a| self.known(a)).collect())
                }
                _ => continue,
            };
            let name = self.prog.structs[b as usize].name.clone();
            if matches!(name.as_str(), "Vec" | "BinaryHeap") && vars.len() == 1 {
                self.unify(item, vars[0]);
            } else if matches!(name.as_str(), "Option" | "Result") && !vars.is_empty() {
                // `Option<Vec<T>>` / `Result<Vec<T>, E>`: 要素は Option<T> / Result<T, E>
                let r0 = self.find(vars[0]);
                let inner = match self.shape[r0 as usize] {
                    Some(Shape::Adt(ib, st, l)) if self.prog.structs[ib as usize].name == "Vec" => {
                        self.adt_vars_of(st, l).first().copied()
                    }
                    Some(Shape::Known(Ty::Struct(s))) if self.prog.structs[self.prog.base_of(s) as usize].name == "Vec" => {
                        self.prog.structs[s as usize].args.first().copied().map(|a| self.known(a))
                    }
                    _ => None,
                };
                if let Some(t) = inner {
                    let mut a = vec![t];
                    a.extend(vars[1..].iter().copied());
                    let it = self.adt(b, a);
                    self.unify(item, it);
                }
            } else if name == "HashMap" && vars.len() == 2 {
                let tb = self.prog.tuple_base(2);
                let t = self.adt(tb, vec![vars[0], vars[1]]);
                self.unify(item, t);
            }
        }
    }

    fn closure_body(&mut self, c: &syn::ExprClosure, vs: &[u32], rv: u32) {
        self.scopes.push(vec![]);
        for (p, &v) in c.inputs.iter().zip(vs) {
            self.pat(p, v, None);
        }
        let saved = self.ret;
        self.ret = rv;
        let b = self.expr(&c.body);
        self.unify(b, rv);
        self.ret = saved;
        self.scopes.pop();
    }

    fn finish_closures(&mut self, mark: usize) {
        while self.pending.len() > mark {
            let (c, vs, rv) = self.pending.remove(mark);
            let saved = std::mem::take(&mut self.defer);
            // SAFETY: c は infer() の間生きている関数の AST の中を指す
            let c = unsafe { &*c };
            self.closure_body(c, &vs, rv);
            self.defer = saved;
        }
    }

    /// 算術・比較の被演算子: 整数 (か未知) への参照なら中身
    fn num_operand(&mut self, v: u32) -> u32 {
        let r = self.find(v);
        match self.shape[r as usize] {
            Some(Shape::Ref(_, i)) => {
                let ri = self.find(i);
                match self.shape[ri as usize] {
                    None => i,
                    Some(Shape::Known(t)) if t.is_int() || t.is_float() || matches!(t, Ty::Char | Ty::Bool) => i,
                    _ => v,
                }
            }
            _ => v,
        }
    }

    /// 式の文脈 (フィールド・メソッド・添字) の自動 deref: 参照と Box を剥がす
    fn deref_expr(&mut self, mut v: u32) -> u32 {
        loop {
            let r = self.find(v);
            match self.shape[r as usize] {
                Some(Shape::Ref(_, i)) | Some(Shape::Bx(i)) => v = i,
                _ => return v,
            }
        }
    }

    fn auto_deref(&mut self, mut v: u32, mut mode: Option<bool>) -> (u32, Option<bool>) {
        loop {
            let r = self.find(v);
            match self.shape[r as usize] {
                Some(Shape::Ref(m, i)) => {
                    v = i;
                    mode = Some(if mode == Some(false) { false } else { m });
                }
                _ => return (v, mode),
            }
        }
    }

    fn bind_name(&mut self, name: String, v: u32, mode: Option<bool>) {
        let v = match mode {
            Some(m) => self.with(Shape::Ref(m, v)),
            None => v,
        };
        self.scopes.last_mut().unwrap().push((name, v));
    }

    /// パターンを型変数 v に当て、束縛を今のスコープに入れる
    fn pat(&mut self, pat: &Pat, v: u32, mode: Option<bool>) {
        match pat {
            Pat::Ident(pi) if crate::codegen::unit_variant_ident(self.prog, pi).is_some() => {
                let path = crate::codegen::unit_variant_ident(self.prog, pi).unwrap();
                let (v, _) = self.auto_deref(v, mode);
                if let Some((sid, vi)) = self.prog.variant_by_path(&path, self.self_sid) {
                    self.pat_fields(v, sid, Some(vi));
                }
            }
            Pat::Ident(pi) if crate::codegen::const_lit_pat(self.prog, pi).is_some() => {
                let lp = crate::codegen::const_lit_pat(self.prog, pi).unwrap();
                self.pat(&lp, v, mode);
            }
            Pat::Ident(pi) => {
                if let Some((_, sub)) = &pi.subpat {
                    self.pat(sub, v, mode);
                }
                let mode = if pi.by_ref.is_some() {
                    Some(pi.mutability.is_some())
                } else {
                    mode
                };
                self.bind_name(pi.ident.to_string(), v, mode);
            }
            Pat::Paren(p) => self.pat(&p.pat, v, mode),
            Pat::Type(p) => self.pat(&p.pat, v, mode),
            Pat::Reference(r) => {
                let i = self.deref(v);
                self.pat(&r.pat, i, None);
            }
            Pat::Or(o) => {
                for c in &o.cases {
                    self.pat(c, v, mode);
                }
            }
            Pat::Lit(l) => {
                let (v, _) = self.auto_deref(v, mode);
                let e = self.lit_var(&l.lit);
                self.unify(v, e);
            }
            Pat::Range(r) => {
                let (v, _) = self.auto_deref(v, mode);
                for e in r.start.iter().chain(r.end.iter()) {
                    let e = self.expr(e);
                    self.unify(v, e);
                }
            }
            Pat::Path(pp) => {
                let (v, _) = self.auto_deref(v, mode);
                if let Some((sid, vi)) = self.prog.variant_by_path(&pp.path, self.self_sid) {
                    self.pat_fields(v, sid, Some(vi));
                }
            }
            Pat::TupleStruct(ts) => {
                let (v, mode) = self.auto_deref(v, mode);
                let fts: Vec<u32> =
                    if let Some((sid, vi)) = self.prog.variant_by_path(&ts.path, self.self_sid) {
                        self.pat_fields(v, sid, Some(vi))
                            .into_iter()
                            .map(|f| f.1)
                            .collect()
                    } else if let Some(sid) = self.prog.struct_by_path(&ts.path, self.self_sid) {
                        self.pat_fields(v, sid, None)
                            .into_iter()
                            .map(|f| f.1)
                            .collect()
                    } else {
                        return;
                    };
                let elems: Vec<&Pat> = ts.elems.iter().collect();
                let rest = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
                for (i, p) in elems.iter().enumerate() {
                    let fi = match rest {
                        Some(r) if i == r => continue,
                        Some(r) if i > r => fts.len().wrapping_sub(elems.len() - i),
                        _ => i,
                    };
                    if let Some(&ft) = fts.get(fi) {
                        self.pat(p, ft, mode);
                    }
                }
            }
            Pat::Tuple(pt) => {
                let (v, mode) = self.auto_deref(v, mode);
                let base = self.prog.tuple_base(pt.elems.len());
                let fts: Vec<u32> = self
                    .pat_fields(v, base, None)
                    .into_iter()
                    .map(|f| f.1)
                    .collect();
                let elems: Vec<&Pat> = pt.elems.iter().collect();
                let rest = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
                for (i, p) in elems.iter().enumerate() {
                    let fi = match rest {
                        Some(r) if i == r => continue,
                        Some(r) if i > r => fts.len().wrapping_sub(elems.len() - i),
                        _ => i,
                    };
                    if let Some(&ft) = fts.get(fi) {
                        self.pat(p, ft, mode);
                    }
                }
            }
            Pat::Struct(ps) => {
                let (v, mode) = self.auto_deref(v, mode);
                let fields =
                    if let Some((sid, vi)) = self.prog.variant_by_path(&ps.path, self.self_sid) {
                        self.pat_fields(v, sid, Some(vi))
                    } else if let Some(sid) = self.prog.struct_by_path(&ps.path, self.self_sid) {
                        self.pat_fields(v, sid, None)
                    } else {
                        return;
                    };
                for fp in &ps.fields {
                    let name = member_name(&fp.member);
                    if let Some(&(_, ft)) = fields.iter().find(|f| f.0 == name) {
                        self.pat(&fp.pat, ft, mode);
                    }
                }
            }
            _ => {}
        }
    }

    fn lookup(&self, n: &str) -> Option<u32> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|(x, _)| x == n)
            .map(|&(_, v)| v)
    }

    fn block(&mut self, b: &syn::Block) -> u32 {
        self.scopes.push(vec![]);
        let mut res = None;
        let n = b.stmts.len();
        for (i, st) in b.stmts.iter().enumerate() {
            match st {
                Stmt::Local(l) => {
                    let (pat, ann) = match &l.pat {
                        Pat::Type(pt) => (&*pt.pat, self.prog.parse_ty(&pt.ty, self.env).ok()),
                        p => (p, None),
                    };
                    let v = match &l.init {
                        Some(init) => self.expr(&init.expr),
                        None => self.var(),
                    };
                    if let Some(t) = ann {
                        let k = self.known(t);
                        self.unify(v, k);
                    }
                    if self.no_record == 0 {
                        self.lets.push((let_key(l), v));
                    }
                    match pat {
                        Pat::Ident(pi) if pi.subpat.is_none() => self
                            .scopes
                            .last_mut()
                            .unwrap()
                            .push((pi.ident.to_string(), v)),
                        p => self.pat(p, v, None),
                    }
                }
                Stmt::Expr(e, semi) => {
                    let v = self.expr(e);
                    if i + 1 == n && semi.is_none() {
                        res = Some(v);
                    }
                    // `{ return None; }` は発散する (型は周りから決まる)
                    if i + 1 == n && semi.is_some() && diverges(e) {
                        res = Some(self.var());
                    }
                }
                Stmt::Macro(m) if i + 1 == n && diverging_macro(&m.mac) => res = Some(self.var()),
                Stmt::Macro(m) if is_fmt_macro(&m.mac) => self.fmt_width_vars(&m.mac),
                _ => {}
            }
        }
        self.scopes.pop();
        res.unwrap_or_else(|| self.known(Ty::Unit))
    }

    /// 式の型変数。値を作る式 (リテラル・構築・呼び出し) は結果の型も記録し、
    /// codegen が期待型のないところで使う (`Pair { a: 1 }.swap()` の 1 を後ろの使用から決める)
    fn expr(&mut self, e: &Expr) -> u32 {
        // 呼び出しの引数のクロージャは、呼び先の境界 (`F: FnMut(Self::Item) -> B`) と単一化してから本体を推論する
        let call = matches!(e, Expr::Call(_) | Expr::MethodCall(_));
        let mark = self.pending.len();
        if call {
            self.defer += 1;
        }
        // ループとラベル付きブロック: break の値の型を集める
        let lname = |l: &Option<syn::Label>| l.as_ref().map(|l| l.name.ident.to_string());
        let frame = match e {
            Expr::Loop(l) => Some((lname(&l.label), false)),
            Expr::While(w) => Some((lname(&w.label), false)),
            Expr::ForLoop(f) => Some((lname(&f.label), false)),
            Expr::Block(b) if b.label.is_some() => Some((lname(&b.label), true)),
            _ => None,
        };
        let v = match frame {
            None => self.expr_inner(e),
            Some((label, is_block)) => {
                let rv = self.var();
                self.brk.push((label, rv, is_block, false));
                let v = self.expr_inner(e);
                let (_, rv, _, broke) = self.brk.pop().unwrap();
                match e {
                    // break のない loop は !
                    Expr::Loop(_) if !broke => self.known(Ty::Never),
                    Expr::Loop(_) => rv,
                    Expr::Block(_) if broke => {
                        if self.resolve(v) != Some(Ty::Never) {
                            self.unify(rv, v);
                        }
                        rv
                    }
                    _ => v,
                }
            }
        };
        if call {
            self.defer -= 1;
            self.finish_closures(mark);
        }
        if matches!(
            e,
            Expr::Lit(_)
                | Expr::Struct(_)
                | Expr::Call(_)
                | Expr::MethodCall(_)
                | Expr::Path(_)
                | Expr::Macro(_)
                | Expr::Try(_)
                | Expr::Index(_)
                | Expr::Field(_)
        ) && self.no_record == 0
        {
            self.lets.push((e as *const Expr as usize, v));
            // (メソッド呼び出しはノード自体でも引けるようにする: codegen の helper が戻り値の型を見る)
            if let Expr::MethodCall(mc) = e {
                self.lets.push((mc as *const syn::ExprMethodCall as usize, v));
            }
            // (関数呼び出しも: `Tr::dec(r)?` の Self を戻り値の型から決める)
            if let Expr::Call(c) = e {
                self.lets.push((c as *const syn::ExprCall as usize, v));
            }
        } else if self.pos_record > 0 && matches!(e, Expr::Lit(_) | Expr::Call(_) | Expr::MethodCall(_) | Expr::Path(_) | Expr::Struct(_) | Expr::Index(_) | Expr::Field(_)) {
            let k = pos_key(e);
            if k != 0 {
                self.lets.push((k, v));
            }
        }
        v
    }

    fn expr_inner(&mut self, e: &Expr) -> u32 {
        use BinOp::*;
        match e {
            // 値としての `a..b` / `a..=b`: prelude の Range<T> / RangeInclusive<T>
            Expr::Range(rg) => {
                let name = if matches!(rg.limits, syn::RangeLimits::Closed(_)) {
                    "RangeInclusive"
                } else if rg.end.is_none() {
                    "RangeFrom"
                } else {
                    "Range"
                };
                let el = self.var();
                for x in rg.start.iter().chain(rg.end.iter()) {
                    let xv = self.expr(x);
                    self.unify(el, xv);
                }
                match self.prog.struct_index.get(name) {
                    Some(&b) => self.adt(b, vec![el]),
                    None => self.var(),
                }
            }
            // `format!(..)`: String
            Expr::Macro(m) if m.mac.path.is_ident("format") && self.prog.struct_index.contains_key("String") => {
                self.fmt_width_vars(&m.mac);
                let ss = self.prog.struct_index["String"];
                self.known(Ty::Struct(ss))
            }
            // `dbg!(e)`: e の型
            Expr::Macro(m) if m.mac.path.is_ident("dbg") => {
                use syn::parse::Parser;
                let Ok(args) = syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated.parse2(m.mac.tokens.clone()) else {
                    return self.var();
                };
                let xs: Vec<Expr> = args.into_iter().collect();
                if xs.is_empty() {
                    return self.var();
                }
                self.no_record += 1;
                self.pos_record += 1;
                let ki = self.keep.len();
                self.keep.push(xs);
                let x: *const Expr = &self.keep[ki][0];
                let v = self.expr(unsafe { &*x });
                self.pos_record -= 1;
                self.no_record -= 1;
                v
            }
            // `matches!(e, pat)`: bool (e は codegen が parse し直すので位置で記録する)
            Expr::Macro(m) if m.mac.path.is_ident("matches") => {
                use syn::parse::Parser;
                let parser = |input: syn::parse::ParseStream| -> syn::Result<Expr> {
                    let e: Expr = input.parse()?;
                    let _: proc_macro2::TokenStream = input.parse()?;
                    Ok(e)
                };
                if let Ok(e) = parser.parse2(m.mac.tokens.clone()) {
                    let ki = self.keep.len();
                    self.keep.push(vec![e]);
                    let x: *const Expr = &self.keep[ki][0];
                    self.no_record += 1;
                    self.pos_record += 1;
                    self.expr(unsafe { &*x });
                    self.pos_record -= 1;
                    self.no_record -= 1;
                }
                self.known(Ty::Bool)
            }
            Expr::Macro(m) if is_fmt_macro(&m.mac) => {
                self.fmt_width_vars(&m.mac);
                self.var()
            }
            // `vec![a, b]` / `vec![x; n]`: Vec<要素の型>
            Expr::Macro(m)
                if m.mac.path.is_ident("vec") && self.prog.struct_index.contains_key("Vec") =>
            {
                use syn::parse::Parser;
                let el = self.var();
                let rep = (|input: syn::parse::ParseStream| {
                    let x: Expr = input.parse()?;
                    let _: syn::Token![;] = input.parse()?;
                    let n: Expr = input.parse()?;
                    Ok((x, n))
                })
                .parse2(m.mac.tokens.clone());
                let xs: Vec<Expr> = match rep {
                    Ok((x, _)) => vec![x],
                    Err(_) => syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated
                        .parse2(m.mac.tokens.clone())
                        .map(|v| v.into_iter().collect())
                        .unwrap_or_default(),
                };
                // (parse し直したノードは推論の後で捨てるので、アドレスで型を記録しない。
                //  解放されたアドレスが codegen の作るノードと重なると別の式の型を引いてしまう)
                self.no_record += 1;
                let ki = self.keep.len();
                self.keep.push(xs);
                for i in 0..self.keep[ki].len() {
                    // (Vec を keep に移しても要素のアドレスは変わらない)
                    let x: *const Expr = &self.keep[ki][i];
                    let v = self.expr(unsafe { &*x });
                    self.unify(el, v);
                }
                self.no_record -= 1;
                let vb = self.prog.struct_index["Vec"];
                self.adt(vb, vec![el])
            }
            Expr::Lit(l) => self.lit_var(&l.lit),
            Expr::Paren(p) => self.expr(&p.expr),
            Expr::Group(g) => self.expr(&g.expr),
            // `e?`: Option<T> / Result<T, E> の T
            Expr::Try(t) => {
                let v = self.expr(&t.expr);
                let (v, _) = self.auto_deref(v, None);
                let r = self.find(v);
                let base = match self.shape[r as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(self.prog.base_of(sid)),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                match base {
                    Some(b)
                        if matches!(
                            self.prog.structs[b as usize].name.as_str(),
                            "Option" | "Result"
                        ) =>
                    {
                        self.adt_view(v, b)[0]
                    }
                    _ => self.var(),
                }
            }
            Expr::Tuple(t) if t.elems.is_empty() => self.known(Ty::Unit),
            Expr::Tuple(t) => {
                let base = self.prog.tuple_base(t.elems.len());
                let args = t
                    .elems
                    .iter()
                    .enumerate()
                    .map(|(i, e)| (i.to_string(), self.expr(e)))
                    .collect();
                self.adt_ctor(base, None, args)
            }
            Expr::Array(a) => {
                let e = self.var();
                for x in &a.elems {
                    let v = self.expr(x);
                    self.unify(e, v);
                }
                self.with(Shape::Arr(e, a.elems.len() as u32))
            }
            Expr::Repeat(r) => {
                let e = self.expr(&r.expr);
                let n = match &*r.len {
                    Expr::Lit(syn::ExprLit {
                        lit: Lit::Int(i), ..
                    }) => i.base10_parse::<u32>().ok(),
                    _ => None,
                };
                match n {
                    Some(n) => self.with(Shape::Arr(e, n)),
                    None => self.var(),
                }
            }
            Expr::Index(ix) => {
                let b = self.expr(&ix.expr);
                if let Expr::Range(rg) = &*ix.index {
                    for x in rg.start.iter().chain(rg.end.iter()) {
                        let v = self.expr(x);
                        let k = self.known(Ty::Usize);
                        self.unify(v, k);
                    }
                    // (`s[a..b]` の s が str / String なら str)
                    let d = self.deref_expr(b);
                    let r = self.find(d);
                    let is_str = match self.shape[r as usize] {
                        Some(Shape::Known(Ty::Str)) => true,
                        Some(Shape::Known(Ty::Struct(sid))) => {
                            self.prog.structs[sid as usize].base.is_none() && self.prog.structs[sid as usize].name == "String"
                        }
                        _ => false,
                    };
                    if is_str {
                        return self.known(Ty::Str);
                    }
                    return match self.elem_of(b) {
                        Some(e) => self.with(Shape::Slc(e)),
                        None => self.var(),
                    };
                }
                let i = self.expr(&ix.index);
                let k = self.known(Ty::Usize);
                self.unify(i, k);
                self.elem_of(b).unwrap_or_else(|| self.var())
            }
            Expr::ForLoop(f) => {
                // アダプタを剥がして要素の型を決める
                let mut e: &Expr = &f.expr;
                let mut adapters = vec![];
                loop {
                    match e {
                        Expr::MethodCall(mc) if mc.args.is_empty() => {
                            adapters.push(mc.method.to_string());
                            e = &mc.receiver;
                        }
                        Expr::Paren(p) => e = &p.expr,
                        _ => break,
                    }
                }
                adapters.reverse();
                let mut iter_item = None;
                let item = if let Expr::Range(rg) = e {
                    let v = self.var();
                    for x in rg.start.iter().chain(rg.end.iter()) {
                        let xv = self.expr(x);
                        self.unify(v, xv);
                    }
                    v
                } else {
                    let (by_ref, base_e) = match e {
                        Expr::Reference(r) => (Some(r.mutability.is_some()), &*r.expr),
                        _ => (None, e),
                    };
                    let bv = self.expr(base_e);
                    let mut mode = by_ref;
                    if mode.is_none() {
                        let r = self.find(bv);
                        if let Some(Shape::Ref(m, _)) = self.shape[r as usize] {
                            mode = Some(m);
                        }
                    }
                    for a in &adapters {
                        match a.as_str() {
                            "iter" => mode = Some(false),
                            "iter_mut" => mode = Some(true),
                            "copied" | "cloned" => mode = None,
                            _ => {}
                        }
                    }
                    match self.elem_of(bv) {
                        Some(el) => match mode {
                            Some(m) => self.with(Shape::Ref(m, el)),
                            None => el,
                        },
                        None => {
                            // Iterator: 式全体の型の `Item`
                            // (型引数が未定のアダプタの列 `(0..n).step_by(2)` でも Item を型変数のまま辿る)
                            let mut fv = if adapters.is_empty() { bv } else { self.expr(&f.expr) };
                            // (ユーザの `impl IntoIterator for &Coll` / `for Coll`: into_iter が返すイテレータの Item)
                            let user_into = match self.resolve(fv) {
                                Some(Ty::Ref(_, Ty::Struct(sid))) => self.prog.method(*sid, "__ref_into_iter"),
                                // (`for x in &c`: c の型で `__ref_into_iter` を探す)
                                // (`for (k, v) in &map`: 無ければ iter / iter_mut (HashMap / BTreeMap / VecDeque など))
                                Some(Ty::Struct(sid)) if by_ref.is_some() => self.prog.method(sid, "__ref_into_iter").or_else(|| {
                                    self.prog.method(sid, if by_ref == Some(true) { "iter_mut" } else { "iter" })
                                }),
                                Some(Ty::Struct(sid)) if self.prog.method(sid, "next").is_none() => self.prog.method(sid, "into_iter"),
                                _ => None,
                            };
                            if let Some(id) = user_into {
                                fv = self.apply_recv(Some(id), vec![], fv);
                            }
                            let it = match self.assoc_var(fv, "Item") {
                                Some(v) => v,
                                None => match self.resolve(fv).and_then(|t| self.prog.assoc_of(t, "Item")) {
                                    Some(t) => self.known(t),
                                    None => self.var(),
                                },
                            };
                            iter_item = Some(it);
                            self.var()
                        }
                    }
                };
                let item = if let Some(it) = iter_item {
                    it
                } else if adapters.iter().any(|a| a == "enumerate") {
                    let base = self.prog.tuple_base(2);
                    let u = self.known(Ty::Usize);
                    self.adt(base, vec![u, item])
                } else {
                    item
                };
                self.scopes.push(vec![]);
                self.pat(&f.pat, item, None);
                self.block(&f.body);
                self.scopes.pop();
                self.known(Ty::Unit)
            }
            Expr::Block(b) => self.block(&b.block),
            Expr::Unsafe(u) => self.block(&u.block),
            Expr::Path(p)
                if p.path.segments.len() >= 2
                    || (p
                        .path
                        .get_ident()
                        .is_some_and(|id| self.lookup(&id.to_string()).is_none())
                        && self.prog.variant_by_path(&p.path, self.self_sid).is_some()) =>
            {
                match self.prog.variant_by_path(&p.path, self.self_sid) {
                    Some((sid, vi)) => self.adt_ctor(sid, Some(vi), vec![]),
                    None => {
                        let segs: Vec<String> = p
                            .path
                            .segments
                            .iter()
                            .map(|s| s.ident.to_string())
                            .collect();
                        match Ty::from_suffix(&segs[0]).or(if segs[0] == "char" {
                            Some(Ty::Char)
                        } else {
                            None
                        }) {
                            Some(_) if segs.len() == 2 && segs[1] == "BITS" => self.known(Ty::U32),
                            Some(t) if segs.len() == 2 => self.known(t),
                            _ => self.var(),
                        }
                    }
                }
            }
            Expr::Match(m) => {
                let sv = self.expr(&m.expr);
                let r = self.var();
                for arm in &m.arms {
                    self.scopes.push(vec![]);
                    self.pat(&arm.pat, sv, None);
                    if let Some((_, g)) = &arm.guard {
                        self.expr(g);
                    }
                    let t = self.expr(&arm.body);
                    self.unify(r, t);
                    self.scopes.pop();
                }
                r
            }
            Expr::Let(l) => {
                // if let / while let の条件。束縛は呼び出し側のスコープに入れる
                let sv = self.expr(&l.expr);
                self.pat(&l.pat, sv, None);
                self.known(Ty::Bool)
            }
            // const / static
            Expr::Path(p)
                if p.path.get_ident().is_some_and(|id| {
                    self.lookup(&id.to_string()).is_none()
                        && self.prog.const_index.contains_key(&id.to_string())
                }) =>
            {
                let ci = self.prog.const_index[&p.path.get_ident().unwrap().to_string()];
                self.known(self.prog.consts[ci as usize].ty)
            }
            Expr::Path(p) => match p
                .path
                .get_ident()
                .and_then(|id| self.lookup(&id.to_string()))
            {
                Some(v) => v,
                // const ジェネリクスの N は usize
                None if p.path.get_ident().is_some_and(|id| {
                    let n = id.to_string();
                    crate::front::const_generic_value(self.env, &n).is_some()
                }) => self.known(Ty::Usize),
                // 値として使われた (ジェネリックでない) 関数は関数ポインタ
                None => match self.prog.fn_by_path(&p.path, self.self_sid) {
                    Some(id)
                        if !self.prog.sigs[id as usize].is_generic()
                            && !self
                                .prog
                                .const_index
                                .contains_key(&p.path.segments.last().unwrap().ident.to_string()) =>
                    {
                        let s = &self.prog.sigs[id as usize];
                        let f = crate::front::intern_fn(crate::front::FnTy {
                            params: s.params.clone(),
                            ret: s.ret,
                        });
                        self.known(Ty::Fn(f))
                    }
                    _ => self.var(),
                },
            },
            Expr::Unary(u) => {
                let v = self.expr(&u.expr);
                match u.op {
                    UnOp::Deref(_) => self.deref(v),
                    _ => v,
                }
            }
            Expr::Reference(r) => {
                let v = self.expr(&r.expr);
                self.with(Shape::Ref(r.mutability.is_some(), v))
            }
            Expr::Field(f) => {
                let b = self.expr(&f.base);
                match &f.member {
                    syn::Member::Named(id) => self.field(b, &id.to_string()),
                    // タプル構造体・タプルのフィールドは "0", "1", .. という名前
                    syn::Member::Unnamed(i) => self.field(b, &i.index.to_string()),
                }
            }
            Expr::Struct(s) => {
                let args: Vec<(String, u32)> = s
                    .fields
                    .iter()
                    .map(|fv| (member_name(&fv.member), self.expr(&fv.expr)))
                    .collect();
                if let Some((sid, vi)) = self.prog.variant_by_path(&s.path, self.self_sid) {
                    self.adt_ctor(sid, Some(vi), args)
                } else if let Some(sid) = self.prog.struct_by_path(&s.path, self.self_sid) {
                    self.adt_ctor(sid, None, args)
                } else {
                    self.var()
                }
            }
            Expr::Binary(b) => {
                let l = self.expr(&b.left);
                let r = self.expr(&b.right);
                // `&i64 * 10` / `acc + x` (x: &i64): 参照は値として計算する (codegen と同じ)
                // (片方が struct かその参照なら演算子のオーバーロード: 参照を数値とみなして外さない)
                let is_adt = |this: &mut Self, v: u32| {
                    let mut t = this.resolve(v);
                    while let Some(Ty::Ref(_, i)) = t {
                        t = Some(*i);
                    }
                    matches!(t, Some(Ty::Struct(_)))
                };
                let adt = is_adt(self, l) || is_adt(self, r);
                let (l, r) = if adt || matches!(b.op, And(_) | Or(_) | Shl(_) | Shr(_) | ShlAssign(_) | ShrAssign(_)) {
                    (l, r)
                } else {
                    (self.num_operand(l), self.num_operand(r))
                };
                match b.op {
                    And(_) | Or(_) => {
                        let k = self.known(Ty::Bool);
                        self.unify(l, k);
                        self.unify(r, k);
                        k
                    }
                    Eq(_) | Ne(_) => {
                        // (`v == &a` / `&s[..] == v`: 片方だけ参照なら中身どうし。Vec と配列・スライスは要素どうし)
                        let (fl, fr) = (self.find(l), self.find(r));
                        match (self.shape[fl as usize], self.shape[fr as usize]) {
                            (Some(Shape::Ref(_, x)), Some(s)) if !matches!(s, Shape::Ref(..)) => self.seq_unify(x, r),
                            (Some(s), Some(Shape::Ref(_, y))) if !matches!(s, Shape::Ref(..)) => self.seq_unify(l, y),
                            // (`w == b"pub"` で w: &[u8]: 参照の中身どうし (スライスと配列は単一化しない))
                            (Some(Shape::Ref(_, x)), Some(Shape::Ref(_, y))) => {
                                let (fx, fy) = (self.find(x), self.find(y));
                                let seqish = |s: &Option<Shape>| matches!(s, Some(Shape::Slc(_) | Shape::Arr(..) | Shape::Known(Ty::Slice(_) | Ty::Array(..))));
                                if seqish(&self.shape[fx as usize]) && seqish(&self.shape[fy as usize]) {
                                    self.seq_unify(x, y)
                                } else {
                                    self.unify(l, r)
                                }
                            }
                            _ => self.seq_unify(l, r),
                        }
                        self.known(Ty::Bool)
                    }
                    Lt(_) | Le(_) | Gt(_) | Ge(_) => {
                        self.unify(l, r);
                        self.known(Ty::Bool)
                    }
                    Shl(_) | Shr(_) => l,
                    ShlAssign(_) | ShrAssign(_) => self.known(Ty::Unit),
                    AddAssign(_) | SubAssign(_) | MulAssign(_) | DivAssign(_) | RemAssign(_)
                    | BitXorAssign(_) | BitAndAssign(_) | BitOrAssign(_) => {
                        self.unify(l, r);
                        self.known(Ty::Unit)
                    }
                    _ => {
                        // 演算子のオーバーロード (`impl Add for V` / `impl Mul<i32> for V` / `impl Add for &M`):
                        // 戻り値は add などのメソッドの型
                        let m = match b.op {
                            Add(_) => "add",
                            Sub(_) => "sub",
                            Mul(_) => "mul",
                            Div(_) => "div",
                            Rem(_) => "rem",
                            BitAnd(_) => "bitand",
                            BitOr(_) => "bitor",
                            BitXor(_) => "bitxor",
                            _ => "",
                        };
                        let lt = self.resolve(l);
                        let found = match lt {
                            Some(Ty::Struct(sid)) if !m.is_empty() => self.prog.method(sid, m).map(|id| (id, l)),
                            Some(Ty::Ref(_, Ty::Struct(sid))) if !m.is_empty() => {
                                self.prog.method(*sid, &format!("__ref_{m}")).map(|id| (id, l))
                            }
                            // (左がまだ分からない `&b * m`: 右が `&M` で `impl Mul for &M` があればそれ)
                            None if !m.is_empty() => match self.resolve(r) {
                                Some(Ty::Ref(_, Ty::Struct(sid))) => self.prog.method(*sid, &format!("__ref_{m}")).map(|id| (id, l)),
                                _ => None,
                            },
                            _ => None,
                        };
                        if let Some((id, recv)) = found {
                            return self.apply_recv(Some(id), vec![r], recv);
                        }
                        self.unify(l, r);
                        l
                    }
                }
            }
            Expr::Assign(a) => {
                let l = self.expr(&a.left);
                let r = self.expr(&a.right);
                self.unify(l, r);
                self.known(Ty::Unit)
            }
            Expr::If(i) => {
                // if let の束縛は then 節だけで見える
                self.scopes.push(vec![]);
                self.expr(&i.cond);
                let t = self.block(&i.then_branch);
                self.scopes.pop();
                match &i.else_branch {
                    Some((_, e)) => {
                        let e = self.expr(e);
                        self.unify(t, e);
                        t
                    }
                    None => self.known(Ty::Unit),
                }
            }
            Expr::While(w) => {
                self.scopes.push(vec![]);
                self.expr(&w.cond);
                self.block(&w.body);
                self.scopes.pop();
                self.known(Ty::Unit)
            }
            Expr::Loop(l) => {
                self.block(&l.body);
                self.known(Ty::Unit)
            }
            Expr::Return(r) => {
                if let Some(e) = &r.expr {
                    let v = self.expr(e);
                    let ret = self.ret;
                    self.unify(v, ret);
                }
                self.known(Ty::Never)
            }
            Expr::Break(b) => {
                let label = b.label.as_ref().map(|l| l.ident.to_string());
                let fi = self.brk.iter().rposition(|f| match &label {
                    Some(n) => f.0.as_deref() == Some(n.as_str()),
                    None => !f.2,
                });
                let v = match &b.expr {
                    Some(e) => Some(self.expr(e)),
                    None => None,
                };
                if let Some(fi) = fi {
                    self.brk[fi].3 = true;
                    let rv = self.brk[fi].1;
                    let v = v.unwrap_or_else(|| self.known(Ty::Unit));
                    self.unify(rv, v);
                }
                self.known(Ty::Never)
            }
            Expr::Continue(_) => self.known(Ty::Never),
            Expr::Call(c) if self.prog.prim_method_call(c, self.self_sid).is_some() => {
                // (作り直した式は推論の終わりまで keep に持ち (後回しのクロージャが指す)、アドレスでは記録しない:
                //  捨てた後のアドレスが codegen の作るノードと重なると別の式の型を引いてしまう。位置で記録する)
                let e = self.prog.prim_method_call(c, self.self_sid).unwrap();
                let ki = self.keep.len();
                self.keep.push(vec![e]);
                let x: *const Expr = &self.keep[ki][0];
                self.no_record += 1;
                self.pos_record += 1;
                let v = self.expr(unsafe { &*x });
                self.pos_record -= 1;
                self.no_record -= 1;
                v
            }
            Expr::Call(c) => {
                if let Expr::Path(p) = &*c.func {
                    // `S::default()` / `<S>::default()` (derive の Default は関数として見えない): S
                    if c.args.is_empty() && p.path.segments.last().is_some_and(|s| s.ident == "default") {
                        let sp = self.prog.strip_mods(&p.path);
                        let pp = sp.as_ref().unwrap_or(&p.path);
                        let ty: Option<syn::Type> = match &p.qself {
                            Some(q) => Some((*q.ty).clone()),
                            None if pp.segments.len() == 2 && pp.segments[0].ident != "Default" => {
                                Some(syn::Type::Path(syn::TypePath { qself: None, path: syn::Path::from(pp.segments[0].clone()) }))
                            }
                            None => None,
                        };
                        if let Some(t) = ty.and_then(|t| self.prog.parse_ty(&t, self.env).ok()).filter(|&t| !self.prog.has_param(t)) {
                            return self.known(t);
                        }
                    }
                    // `u32::from_le_bytes(x)`: 引数は [u8; 4]
                    let segs = &p.path.segments;
                    if segs.len() == 2
                        && c.args.len() == 1
                        && matches!(segs[1].ident.to_string().as_str(), "from_le_bytes" | "from_be_bytes" | "from_ne_bytes")
                    {
                        if let Some(t) = Ty::prim(&segs[0].ident.to_string()).filter(|t| t.is_int()) {
                            let a = self.expr(&c.args[0]);
                            let k = self.known(Ty::Array(intern(Ty::U8), t.bits() / 8));
                            self.unify(a, k);
                            return self.known(t);
                        }
                    }
                    if let Some((sid, vi)) = self.prog.variant_by_path(&p.path, self.self_sid) {
                        let args = c
                            .args
                            .iter()
                            .enumerate()
                            .map(|(i, a)| (i.to_string(), self.expr(a)))
                            .collect();
                        return self.adt_ctor(sid, Some(vi), args);
                    }
                    let local = p
                        .path
                        .get_ident()
                        .is_some_and(|id| self.lookup(&id.to_string()).is_some());
                    if !local && self.prog.fn_by_path(&p.path, self.self_sid).is_none() {
                        if let Some(sid) = self.prog.struct_by_path(&p.path, self.self_sid) {
                            let args = c
                                .args
                                .iter()
                                .enumerate()
                                .map(|(i, a)| (i.to_string(), self.expr(a)))
                                .collect();
                            return self.adt_ctor(sid, None, args);
                        }
                    }
                }
                let callee = match &*c.func {
                    Expr::Path(p)
                        if p.path
                            .get_ident()
                            .is_some_and(|id| self.lookup(&id.to_string()).is_some()) =>
                    {
                        None
                    }
                    Expr::Path(p) => self.prog.fn_by_path(&p.path, self.self_sid),
                    _ => None,
                };
                let args: Vec<u32> = c.args.iter().map(|a| self.expr(a)).collect();
                // `Box::new(x)` は `&mut T` (所有する参照として扱う)
                if let Expr::Path(p) = &*c.func {
                    let n = p.path.segments.len();
                    if callee.is_none()
                        && n >= 2
                        && p.path.segments[n - 1].ident == "new"
                        && p.path.segments[n - 2].ident == "Box"
                        && args.len() == 1
                    {
                        return self.with(Shape::Bx(args[0]));
                    }
                }
                if callee.is_none() {
                    // クロージャ (の参照) / `&dyn Fn` の呼び出し
                    let f = self.expr(&c.func);
                    return match self.callable(f, args.len()) {
                        Some((ps, r)) => {
                            for (a, p) in args.into_iter().zip(ps) {
                                self.unify(a, p);
                            }
                            r
                        }
                        None => self.var(),
                    };
                }
                self.apply(callee, args, 0)
            }
            Expr::Closure(c) => {
                // 引数・戻り値を型変数にして本体を推論する (キャプチャは外のスコープを見る)
                let st = self.adt_args.len() as u32;
                let n = c.inputs.len() as u32;
                let mut vs = vec![];
                for p in &c.inputs {
                    let v = match crate::closure::param(p).1 {
                        Some(t) => match self.prog.parse_ty(t, self.env) {
                            Ok(t) => self.known(t),
                            Err(_) => self.var(),
                        },
                        None => self.var(),
                    };
                    vs.push(v);
                }
                let rv = match &c.output {
                    syn::ReturnType::Type(_, t) => match self.prog.parse_ty(t, self.env) {
                        Ok(t) => self.known(t),
                        Err(_) => self.var(),
                    },
                    syn::ReturnType::Default => self.var(),
                };
                self.adt_args.extend(vs.iter().copied().chain([rv]));
                // (クロージャの struct を後で解けるように、キャプチャの型変数を覚えておく)
                if self.clo_ctx.is_none() {
                    if let Some((r, base, b)) = self.clo_root.take() {
                        let mut m = crate::fxhash::FxMap::default();
                        // SAFETY: b は infer の間生きている関数の本体を指す
                        for (i, cl) in crate::closure::closures_of(unsafe { &*b }).iter().enumerate() {
                            m.insert(crate::closure::closure_key(cl), base + i as u32);
                        }
                        self.clo_ctx = Some((r, m));
                    }
                }
                if let Some((root, keys)) = &self.clo_ctx {
                    if let Some(fid) = keys.get(&crate::closure::closure_key(c)).and_then(|k| self.prog.closure_by.get(&(*root, *k))) {
                        if let Some(cd) = self.prog.closures.get(fid) {
                            let caps: Option<Vec<u32>> = cd.captures.iter().map(|n| self.lookup(n)).collect();
                            let free: Vec<usize> = c
                                .inputs
                                .iter()
                                .enumerate()
                                .filter(|(_, p)| crate::closure::param(p).1.is_none())
                                .map(|(i, _)| i)
                                .collect();
                            if let Some(caps) = caps {
                                self.clo_info.insert(st, (*fid, caps, free));
                            }
                        }
                    }
                }
                // codegen がクロージャの型引数を決めるのに使う: 引数 (パターンのアドレス) と戻り値 (式のアドレス)
                if self.no_record == 0 {
                    for (p, &v) in c.inputs.iter().zip(&vs) {
                        self.lets.push((p as *const Pat as usize, v));
                    }
                    self.lets.push((c as *const syn::ExprClosure as usize, rv));
                }
                if self.defer > 0 {
                    self.pending.push((c as *const syn::ExprClosure, vs, rv));
                } else {
                    self.closure_body(c, &vs, rv);
                }
                self.with(Shape::Clo(st, n))
            }
            // 値としての `[a, b].into_iter()`: Vec の IntoIter (codegen が vec![..] にする)
            Expr::MethodCall(mc)
                if mc.method == "into_iter"
                    && mc.args.is_empty()
                    && matches!(&*mc.receiver, Expr::Array(_))
                    && self.prog.struct_index.contains_key("IntoIter") =>
            {
                let Expr::Array(a) = &*mc.receiver else { unreachable!() };
                let el = self.var();
                for x in &a.elems {
                    let v = self.expr(x);
                    self.unify(el, v);
                }
                let ib = self.prog.struct_index["IntoIter"];
                self.adt(ib, vec![el])
            }
            // `v.get(a..b)` (Vec / 配列 / スライス): Option<&[T]>
            Expr::MethodCall(mc)
                if (mc.method == "get" || mc.method == "get_mut")
                    && matches!(mc.args.first(), Some(Expr::Range(_)))
                    && {
                        let r = self.expr(&mc.receiver);
                        self.elem_of(r).is_some()
                    } =>
            {
                let r = self.expr(&mc.receiver);
                let el = self.elem_of(r).unwrap();
                if let Some(Expr::Range(rg)) = mc.args.first() {
                    for x in rg.start.iter().chain(rg.end.iter()) {
                        let v = self.expr(x);
                        let k = self.known(Ty::Usize);
                        self.unify(v, k);
                    }
                }
                let m = mc.method == "get_mut";
                let sl = self.with(Shape::Slc(el));
                let rr = self.with(Shape::Ref(m, sl));
                self.option_of(rr)
            }
            Expr::MethodCall(mc)
                if {
                    let r = self.expr_peek_elem(&mc.receiver);
                    r.is_some()
                } =>
            {
                let r = self.expr(&mc.receiver);
                let el = self.elem_of(r).unwrap();
                let args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                match mc.method.to_string().as_str() {
                    "len" => self.known(Ty::Usize),
                    "is_empty" | "contains" => self.known(Ty::Bool),
                    // `s.try_into()`: Result<[T; N], TryFromSliceError> (N は使われ方から)
                    "try_into" if self.prog.struct_index.contains_key("TryFromSliceError") => {
                        let rb = self.prog.struct_index["Result"];
                        let eb = self.prog.struct_index["TryFromSliceError"];
                        let t = self.var();
                        let e = self.known(Ty::Struct(eb));
                        self.adt(rb, vec![t, e])
                    }
                    // `s.get(a..b)`: Option<&[T]>
                    "get" | "get_mut" if matches!(mc.args.first(), Some(Expr::Range(_))) => {
                        let m = mc.method == "get_mut";
                        let sl = self.with(Shape::Slc(el));
                        let rr = self.with(Shape::Ref(m, sl));
                        self.option_of(rr)
                    }
                    "get" | "get_mut" | "first" | "last" => {
                        for a in args {
                            let k = self.known(Ty::Usize);
                            self.unify(a, k);
                        }
                        let m = mc.method == "get_mut";
                        let rr = self.with(Shape::Ref(m, el));
                        self.option_of(rr)
                    }
                    // (添字の引数は usize)
                    "swap" | "truncate" | "rotate_left" | "rotate_right" => {
                        for a in args {
                            let k = self.known(Ty::Usize);
                            self.unify(a, k);
                        }
                        self.known(Ty::Unit)
                    }
                    "split_at" | "split_at_mut" => {
                        for a in args {
                            let k = self.known(Ty::Usize);
                            self.unify(a, k);
                        }
                        self.var()
                    }
                    // 値としての `xs.iter()` / `iter_mut()`: prelude の Iter<T> / IterMut<T>
                    "iter" | "iter_mut" => {
                        let name = if mc.method == "iter" { "Iter" } else { "IterMut" };
                        match self.prog.struct_index.get(name) {
                            Some(&b) => self.adt(b, vec![el]),
                            None => self.var(),
                        }
                    }
                    "as_ptr" => {
                        let k = self.resolve(el).map(|t| Ty::Ptr(false, intern(t)));
                        match k {
                            Some(t) => self.known(t),
                            None => self.var(),
                        }
                    }
                    "as_mut_ptr" => {
                        let k = self.resolve(el).map(|t| Ty::Ptr(true, intern(t)));
                        match k {
                            Some(t) => self.known(t),
                            None => self.var(),
                        }
                    }
                    _ => self.var(),
                }
            }
            Expr::MethodCall(mc) if is_builtin_scalar_method(&mc.method.to_string()) => {
                let r = self.expr(&mc.receiver);
                let rv = self.deref_expr(r);
                let args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                // (String::as_bytes など、struct が同じ名前のメソッドを持つならそれ)
                let root = self.find(rv);
                let sid = match self.shape[root as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                if let Some(id) = sid.and_then(|s| self.prog.method(s, &mc.method.to_string())) {
                    return self.apply_recv(Some(id), args, rv);
                }
                self.scalar_method(mc, r, rv, args)
            }
            Expr::MethodCall(mc) => {
                let r = self.expr(&mc.receiver);
                let rv = self.deref_expr(r);
                let root = self.find(rv);
                let sid = match self.shape[root as usize] {
                    Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                    Some(Shape::Adt(b, _, _)) => Some(b),
                    _ => None,
                };
                let n = mc.method.to_string();
                let mut callee = sid.and_then(|sid| {
                    self.prog
                        .method(sid, &n)
                        .or_else(|| self.prog.default_method(sid, &n))
                });
                // メソッドがなく `deref` があれば、deref した先 (`rc.borrow()` の RefCell) で探す
                let mut rv = rv;
                let mut sid = sid;
                let mut hops = 0;
                while callee.is_none() && hops < 4 {
                    let Some(s0) = sid else { break };
                    let Some(did) = self.prog.method(s0, "deref") else { break };
                    if !matches!(n.as_str(), "clone" | "to_string" | "cmp" | "partial_cmp" | "eq") {
                        let r = self.apply_recv(Some(did), vec![], rv);
                        rv = self.deref_expr(r);
                        let root = self.find(rv);
                        sid = match self.shape[root as usize] {
                            Some(Shape::Known(Ty::Struct(sid))) => Some(sid),
                            Some(Shape::Adt(b, _, _)) => Some(b),
                            _ => None,
                        };
                        callee = sid.and_then(|sid| {
                            self.prog.method(sid, &n).or_else(|| self.prog.default_method(sid, &n))
                        });
                    } else {
                        break;
                    }
                    hops += 1;
                }
                // derive(Clone) の clone (メソッドの本体は codegen が作る): 受け手 (参照を外した) と同じ型
                if callee.is_none() && n == "clone" && mc.args.is_empty() && sid.is_some() {
                    return rv;
                }
                let args: Vec<u32> = mc.args.iter().map(|a| self.expr(a)).collect();
                // `&dyn Trait` のメソッド: trait の宣言のシグネチャ
                let mut dt = self.resolve(rv);
                while let Some(Ty::Ref(_, i)) = dt {
                    dt = Some(*i);
                }
                if let Some(Ty::Dyn(tr)) = dt {
                    let decl = self.prog.traits.get(tr).and_then(|ds| {
                        ds.iter()
                            .copied()
                            .find(|&d| self.prog.names[d as usize].rsplit("::").next() == Some(&n))
                    });
                    if let Some(d) = decl {
                        let sig = &self.prog.sigs[d as usize];
                        let (ps, ret) = (sig.params[1..].to_vec(), sig.ret);
                        for (a, p) in args.into_iter().zip(ps) {
                            if !self.prog.has_param(p) {
                                let k = self.known(p);
                                self.unify(a, k);
                            }
                        }
                        if !self.prog.has_param(ret) {
                            return self.known(ret);
                        }
                    }
                    return self.var();
                }
                // `x.to_string()` (Display): String
                if callee.is_none() && n == "to_string" && args.is_empty() {
                    if let Some(&ss) = self.prog.struct_index.get("String") {
                        return self.known(Ty::Struct(ss));
                    }
                }
                // `parts.join(", ")` / `.concat()`: String (要素が Vec なら、その Vec)
                if callee.is_none() && matches!(n.as_str(), "join" | "concat") && args.len() <= 1 {
                    let mut t = self.resolve(rv);
                    while let Some(Ty::Ref(_, i)) = t {
                        t = Some(*i);
                    }
                    let elem = match t {
                        Some(Ty::Array(e, _)) | Some(Ty::Slice(e)) => Some(*e),
                        Some(Ty::Struct(s)) if self.prog.structs[s as usize].name.starts_with("Vec") => {
                            self.prog.structs[s as usize].args.first().copied()
                        }
                        _ => None,
                    };
                    if let Some(e @ Ty::Struct(es)) = elem {
                        if n == "concat" && self.prog.base_of(es) == self.prog.struct_index.get("Vec").copied().unwrap_or(u32::MAX) {
                            return self.known(e);
                        }
                    }
                    // (列でない受け手 (`path.join("x")`) は trait のメソッドとして下で見る)
                    if elem.is_some() || t.is_none() {
                        if let Some(&ss) = self.prog.struct_index.get("String") {
                            return self.known(Ty::Struct(ss));
                        }
                    }
                }
                // 組み込みの比較 (`3.cmp(&5)` / derive した型の cmp): Ordering / Option<Ordering>
                if callee.is_none() && matches!(n.as_str(), "cmp" | "partial_cmp") && args.len() == 1 {
                    if let Some(&os) = self.prog.struct_index.get("Ordering") {
                        let o = self.known(Ty::Struct(os));
                        return if n == "cmp" { o } else { self.option_of(o) };
                    }
                }
                // スライス・配列の受け手 (`v.swap(i, j)` の v: &mut [T]): Vec の同名メソッドのシグネチャで推論する
                if callee.is_none() && sid.is_none() {
                    let mut t = self.resolve(rv);
                    while let Some(Ty::Ref(_, i)) = t {
                        t = Some(*i);
                    }
                    let root = self.find(rv);
                    let elem = match (t, self.shape[root as usize]) {
                        (Some(Ty::Slice(e)) | Some(Ty::Array(e, _)), _) => Some(self.known(*e)),
                        (_, Some(Shape::Slc(e)) | Some(Shape::Arr(e, _))) => Some(e),
                        _ => None,
                    };
                    if let (Some(e), Some(&vb)) = (elem, self.prog.struct_index.get("Vec")) {
                        if let Some(id) = self.prog.method(vb, &n) {
                            if !matches!(n.as_str(), "len" | "is_empty" | "iter" | "iter_mut" | "into_iter") {
                                let vv = self.adt(vb, vec![e]);
                                return self.apply_recv(Some(id), args.clone(), vv);
                            }
                        }
                    }
                    // 受け手の型が分からない (ジェネリックな `&mut [T]` など) ときも、添字の引数 (`swap(i, j)`) は usize
                    if t.is_none() && matches!(n.as_str(), "swap" | "split_at" | "split_at_mut" | "truncate" | "rotate_left" | "rotate_right" | "split_off") {
                        for &a in &args {
                            let u = self.known(Ty::Usize);
                            self.unify(a, u);
                        }
                    }
                }
                if callee.is_none() && sid.is_none() {
                    // プリミティブ型への trait 実装のメソッド
                    if let Some(t) = self.resolve(rv) {
                        if let Some(id) = self.prog.prim_method(t, &n) {
                            return self.apply_recv(Some(id), args, rv);
                        }
                    }
                    // struct でない受け手: 整数などの組み込みメソッド (min / max など)。
                    // 受け手の型が分からない未知のメソッドは分からないまま
                    if self.resolve(rv).is_none()
                        && !is_builtin_scalar_method(&n)
                        && !matches!(n.as_str(), "min" | "max" | "len" | "is_empty")
                    {
                        return self.var();
                    }
                    return self.scalar_method(mc, r, rv, args);
                }
                // `v.extend(arg)` (Vec / BinaryHeap): arg の要素 (参照なら中身) が v の要素
                if n == "extend" && args.len() == 1 {
                    if let Some(b) = sid {
                        let bn = self.prog.structs[self.prog.base_of(b) as usize].name.as_str();
                        if matches!(bn, "Vec" | "BinaryHeap") {
                            let root = self.find(rv);
                            let elem = match self.shape[root as usize] {
                                Some(Shape::Adt(_, st, l)) => self.adt_vars_of(st, l).first().copied(),
                                Some(Shape::Known(Ty::Struct(s))) => {
                                    self.prog.structs[s as usize].args.first().copied().map(|t| self.known(t))
                                }
                                _ => None,
                            };
                            let known_elem = elem.is_some_and(|ev| self.resolve(ev).is_some());
                            if let (Some(ev), Some(at), false) = (elem, self.resolve(args[0]), known_elem) {
                                if let Some(it) = self.extend_item(at) {
                                    // (`&T` の要素は T を足す。`&str` / `&[T]` はそれ自体が要素)
                                    let it = match it {
                                        Ty::Ref(_, i) if !matches!(i, Ty::Str | Ty::Slice(_) | Ty::Dyn(_)) => *i,
                                        t => t,
                                    };
                                    let k = self.known(it);
                                    self.unify(ev, k);
                                }
                            }
                            return self.known(Ty::Unit);
                        }
                    }
                }
                // String にないメソッド (`s.to_uppercase()`): &str の helper
                if callee.is_none() {
                    if let Some(b) = sid {
                        if self.prog.structs[self.prog.base_of(b) as usize].name == "String" {
                            let h = if n == "split" && args.first().is_some_and(|&a| self.resolve(a) != Some(Ty::Char)) {
                                "split_str".to_string()
                            } else if n == "split_whitespace" {
                                "split_ws".to_string()
                            } else if matches!(mc.args.first(), Some(Expr::Closure(_)))
                                && self.prog.index.contains_key(&format!("__exr_str_{n}_fn"))
                            {
                                format!("{n}_fn")
                            } else {
                                n.clone()
                            };
                            if let Some(&id) = self.prog.index.get(&format!("__exr_str_{h}")) {
                                return self.apply_helper(id, mc, args);
                            }
                        }
                    }
                }
                self.turbofish = mc.turbofish.as_ref().map(|t| t as *const _);
                let r = self.apply_recv(callee, args, rv);
                self.turbofish = None;
                if n == "collect" {
                    self.collects.push((rv, r));
                }
                r
            }
            _ => self.expr_rest(e),
        }
    }

    /// 整数 / &str / 生ポインタの組み込みメソッドの型
    fn scalar_method(&mut self, mc: &syn::ExprMethodCall, r: u32, rv: u32, args: Vec<u32>) -> u32 {
        let root = self.find(rv);
        let recv_ty = match self.shape[root as usize] {
            Some(Shape::Known(t)) => Some(t),
            _ => None,
        };
        let name = mc.method.to_string();
        // prelude の `__exr_str_<m>` / `__exr_char_<m>` (シグネチャで推論する)
        let pre = match self.shape[root as usize] {
            Some(Shape::Known(Ty::Str)) => Some("__exr_str_"),
            Some(Shape::Known(Ty::Char)) => Some("__exr_char_"),
            _ => None,
        };
        if let Some(pre) = pre {
            let h = if name == "split" && pre == "__exr_str_" && args.first().is_some_and(|&a| self.resolve(a) != Some(Ty::Char)) {
                "split_str".to_string()
            } else if name == "split_whitespace" {
                "split_ws".to_string()
            } else if matches!(mc.args.first(), Some(Expr::Closure(_)))
                && self.prog.index.contains_key(&format!("{pre}{name}_fn"))
            {
                format!("{name}_fn")
            } else {
                name.clone()
            };
            if let Some(&id) = self.prog.index.get(&format!("{pre}{h}")) {
                let builtin = matches!(name.as_str(), "len" | "is_empty" | "as_bytes" | "as_ptr");
                if !builtin {
                    return self.apply_helper(id, mc, args);
                }
            }
        }
        // 整数の `to_le_bytes` など: [u8; N]
        if let Some(t) = recv_ty.filter(|t| t.is_int()) {
            if matches!(name.as_str(), "to_le_bytes" | "to_be_bytes" | "to_ne_bytes") {
                return self.known(Ty::Array(intern(Ty::U8), t.bits() / 8));
            }
        }
        // 浮動小数点のメソッド (bool を返すもの / ビット列 / Ordering 以外は同じ型)
        if let Some(t) = recv_ty.filter(|t| t.is_float()) {
            return match name.as_str() {
                "is_nan" | "is_finite" | "is_infinite" | "is_sign_negative" | "is_sign_positive" | "is_normal" => {
                    self.known(Ty::Bool)
                }
                "to_bits" => self.known(if t == Ty::F32 { Ty::U32 } else { Ty::U64 }),
                "total_cmp" => match self.prog.struct_index.get("Ordering") {
                    Some(&o) => self.known(Ty::Struct(o)),
                    None => self.var(),
                },
                _ => {
                    for a in args {
                        let _ = a;
                    }
                    self.known(t)
                }
            };
        }
        // &str
        if let Some(Shape::Known(Ty::Str)) = self.shape[root as usize] {
            return match name.as_str() {
                "len" => self.known(Ty::Usize),
                "is_empty" => self.known(Ty::Bool),
                "as_bytes" => self.known(Ty::Ref(false, intern(Ty::Slice(intern(Ty::U8))))),
                "as_ptr" => self.known(Ty::Ptr(false, intern(Ty::U8))),
                // prelude の &str の関数 (split などのイテレータは分からないまま)
                "trim" | "trim_start" | "trim_end" => self.known(Ty::Ref(false, intern(Ty::Str))),
                "starts_with" | "ends_with" | "contains" => self.known(Ty::Bool),
                "to_string" | "to_owned" => match self.prog.struct_index.get("String") {
                    Some(&s) => self.known(Ty::Struct(s)),
                    None => self.var(),
                },
                "find" => {
                    let u = self.known(Ty::Usize);
                    self.option_of(u)
                }
                _ => self.var(),
            };
        }
        // 生ポインタ
        if let Some(Ty::Ptr(_, e)) = self.resolve(r) {
            return match name.as_str() {
                "add" | "sub" | "offset" | "wrapping_add" | "wrapping_sub" | "wrapping_offset" => r,
                "is_null" => self.known(Ty::Bool),
                "read" | "read_volatile" | "read_unaligned" => self.known(*e),
                _ => self.known(Ty::Unit),
            };
        }
        let _ = recv_ty;
        match name.as_str() {
            "wrapping_add" | "wrapping_sub" | "wrapping_mul" | "min" | "max" | "saturating_add"
            | "saturating_sub" => {
                for a in args {
                    self.unify(a, rv);
                }
                rv
            }
            "rem_euclid" | "div_euclid" => {
                for a in args {
                    self.unify(a, rv);
                }
                rv
            }
            "overflowing_add" | "overflowing_sub" | "overflowing_mul" | "overflowing_pow" => {
                for a in args {
                    let k = if name == "overflowing_pow" { self.known(Ty::U32) } else { rv };
                    self.unify(a, k);
                }
                let b = self.known(Ty::Bool);
                let tb = self.prog.tuple_base(2);
                self.adt(tb, vec![rv, b])
            }
            "checked_pow" => {
                for a in args {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                self.option_of(rv)
            }
            "leading_ones" | "trailing_ones" => self.known(Ty::U32),
            "checked_abs" | "checked_neg" => self.option_of(rv),
            // `x.try_into()`: Result<T, TryFromIntError> (T は使われ方から)
            "try_into" => match (self.prog.struct_index.get("Result"), self.prog.struct_index.get("TryFromIntError")) {
                (Some(&rb), Some(&eb)) => {
                    let t = self.var();
                    let e = self.known(Ty::Struct(eb));
                    self.adt(rb, vec![t, e])
                }
                _ => self.var(),
            },
            "wrapping_neg" | "abs" | "signum" | "swap_bytes" | "to_ascii_uppercase"
            | "to_ascii_lowercase" | "next_power_of_two" | "isqrt" => rv,
            "pow" | "rotate_left" | "rotate_right" | "wrapping_pow" | "saturating_pow" => {
                for a in args {
                    let k = self.known(Ty::U32);
                    self.unify(a, k);
                }
                rv
            }
            "checked_add" | "checked_sub" | "checked_mul" | "checked_div" | "checked_rem" => {
                for a in args {
                    self.unify(a, rv);
                }
                self.option_of(rv)
            }
            "count_ones" | "count_zeros" | "leading_zeros" | "trailing_zeros" => {
                self.known(Ty::U32)
            }
            "abs_diff" => self.var(),
            _ => self.known(Ty::Bool),
        }
    }

    fn expr_rest(&mut self, e: &Expr) -> u32 {
        match e {
            Expr::Cast(c) => {
                self.expr(&c.expr);
                match self.prog.parse_ty(&c.ty, self.env) {
                    Ok(t) => self.known(t),
                    Err(_) => self.var(),
                }
            }
            _ => self.var(),
        }
    }
}

/// 必ず発散する式 (`return` / `break` / `continue` / `panic!(..)`)
fn diverges(e: &Expr) -> bool {
    match e {
        Expr::Return(_) | Expr::Break(_) | Expr::Continue(_) => true,
        Expr::Macro(m) => diverging_macro(&m.mac),
        _ => false,
    }
}

fn diverging_macro(m: &syn::Macro) -> bool {
    m.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "panic" | "unreachable" | "todo" | "unimplemented"))
}

fn is_fmt_macro(m: &syn::Macro) -> bool {
    m.path.segments.last().is_some_and(|s| {
        matches!(s.ident.to_string().as_str(), "print" | "println" | "eprint" | "eprintln" | "write" | "writeln" | "format" | "panic")
    })
}

/// マクロの引数の式の記録に使う、ソース上の位置のキー (アドレスのキーと重ならないよう最上位ビットを立てる)
pub fn pos_key(e: &Expr) -> usize {
    use syn::spanned::Spanned;
    let sp = e.span();
    let (a, b) = (sp.start(), sp.end());
    // (作り直したノード (call_site) は位置で区別できない: 記録しない)
    let cs = proc_macro2::Span::call_site();
    if a == cs.start() && b == cs.end() {
        return 0;
    }
    (1usize << 63)
        | ((a.line & 0xFFFFF) << 43)
        | ((a.column & 0x7FF) << 32)
        | ((b.line & 0xFFFFF) << 12)
        | (b.column & 0xFFF)
}
