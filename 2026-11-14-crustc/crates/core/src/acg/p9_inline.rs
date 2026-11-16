//! 小さい関数のインライン展開 (`-O` のとき)。
//!
//! minclif は IR を持たず、命令をすぐ機械語にする。だから構文木の段階で展開する: 呼び出しの引数を ABI の形
//! (値渡しの集成体はコピーしたアドレス、sret の戻り値は書き込む先) まで作ったところで、呼ぶ代わりに呼び先の本体を
//! 同じ FunctionBuilder にコード生成する。本体の引数は func_params の代わりにその値に束縛し、`return` / 末尾の値は
//! 続きのブロックへの jump にする (emit_return が見る)。呼び先の型推論・変数・drop の状態は入れ替えてから戻す。
//!
//! 展開するのは小さい関数: クロージャ・マクロを含まず、本体の構文木が小さく、特別な本体 (drop glue など) や
//! 呼んだ場所を出す panic (unwrap など) でないもの。ユーザの関数を展開した呼ぶ側は、キャッシュのキーに呼び先のアイテムの
//! ハッシュと行を入れる (driver2::fn_key)。依存するクレートの関数は展開しない (本体はインターフェースのハッシュに入らない)。
//! 展開した中でもう 1 段まで展開する

use super::*;

/// 展開してよい関数の本体 (関数 ID → 構文木)
#[derive(Default)]
pub struct Inline {
    pub bodies: crate::fxhash::FxMap<u32, ItemFn>,
    /// ユーザの関数の、アイテムの開始行 - 1 (panic の場所の基準)。prelude の関数はない
    pub user_lines: crate::fxhash::FxMap<u32, u32>,
    /// 呼び先 (インスタンス) ごとの型推論の結果 (呼ぶ側に依らないので 1 回だけ)
    pub inferred: Mutex<crate::fxhash::FxMap<u32, Inferred>>,
    /// minclif の代わりに使うバックエンド (driver2 の Cranelift バックエンド: crate::clif_backend::compile)
    pub cl: Option<ClBackend>,
}

/// 記録した命令の列から (機械語, 再配置, trap) を作るバックエンド (引数: 記録, 入口で 0 にするスロット, -O か)
pub type ClBackend = fn(&crate::minclif::FunctionBuilder, &[crate::minclif::StackSlot], bool) -> Result<(Vec<u8>, Vec<crate::minclif::Reloc>, Vec<(u32, u8)>), String>;

type Inferred = (crate::ast_infer::LetTypes, crate::ast_infer::MacroArgs, crate::ast_infer::TextTypes);

/// 本体の構文木の大きさの上限 (Debug の文字列の長さ)
const MAX_BODY: usize = 1600;
/// 1 関数の中で展開する回数の上限
const MAX_PER_FN: u32 = 400;

/// 展開してよい形か (本体の構文木だけで決める)
pub fn inline_candidate(f: &ItemFn) -> bool {
    if f.attrs.iter().any(|a| a.contains("inline(never)") || a.contains("track_caller") || a.contains("rustc_inherit_overflow_checks")) {
        return false;
    }
    let d = format!("{:?}", f.block);
    d.len() <= MAX_BODY && !d.contains("Closure(") && !d.contains("Macro(") && !d.contains("Item(")
}

impl<'p> Cx<'p> {
    /// target を args (ABI の形の引数) で呼ぶ代わりに展開する。展開しなければ None。
    /// 展開したら Some(戻り値) (sret なら None の戻り値: 呼ぶ側が書き込む先を持っている)
    pub(crate) fn try_inline(&mut self, target: u32, sig: &FnSig, args: &[Value]) -> R<Option<Option<Value>>> {
        if !self.prog.opt || self.inline_depth >= 2 || self.inlined >= MAX_PER_FN || sig.ret == Ty::Never {
            return Ok(None);
        }
        let (body_id, env) = {
            let g = self.insts.lock().unwrap();
            (g.body_of(target), g.env(self.prog, target))
        };
        let Some(f) = self.inl.bodies.get(&body_id) else { return Ok(None) };
        // (自分自身は展開しない)
        if body_id == self.root {
            return Ok(None);
        }
        let cached = self.inl.inferred.lock().unwrap().get(&target).cloned();
        let (lets, margs, texts) = match cached {
            Some(x) => x,
            None => {
                let ((lets, _, _), margs, texts) =
                    crate::ast_infer::infer_ctx_margs(self.prog, f.sig.recv.is_some(), &f.sig.params, &f.block, sig, &env, &[], Some(sig.ret), Some((body_id, 0)));
                self.inl.inferred.lock().unwrap().insert(target, (lets.clone(), margs.clone(), texts.clone()));
                (lets, margs, texts)
            }
        };
        self.inlined += 1;
        // 続き (戻り値はブロックの引数)
        let cont = self.b.create_block();
        let sret = is_sret(sig);
        let ret_ty = if sret { None } else { clif_ty(sig.ret) };
        let rv = ret_ty.map(|t| self.b.append_block_param(cont, t));
        let entry = self.b.create_block();
        self.b.ins().jump(entry, &[]);
        self.b.switch_to_block(entry);
        // 呼び先の状態にする
        let s_scopes = std::mem::replace(&mut self.scopes, vec![vec![]]);
        let s_loops = std::mem::take(&mut self.loops);
        let s_label = self.pending_label.take();
        let s_ret = std::mem::replace(&mut self.ret, sig.ret);
        let s_main = std::mem::replace(&mut self.is_main, false);
        let user_lb = self.inl.user_lines.get(&body_id).copied();
        let s_user = std::mem::replace(&mut self.user_code, user_lb.is_some());
        let s_ovf = std::mem::replace(&mut self.overflow_checks, self.prog.overflow_checks && user_lb.is_some());
        let lb0 = self.line_base;
        let s_lbase = std::mem::replace(&mut self.line_base, user_lb.unwrap_or(lb0));
        let s_ipb = self.index_panic_block.take();
        let s_caller = std::mem::replace(&mut self.caller_loc, false);
        let s_self = std::mem::replace(&mut self.self_sid, env.self_sid);
        let s_sret = std::mem::replace(&mut self.sret, if sret { args.first().copied() } else { None });
        let s_lets = std::mem::replace(&mut self.lets, lets);
        let s_taken = std::mem::replace(&mut self.addr_taken, addr_taken(&f.block));
        let s_env = std::mem::replace(&mut self.env, env);
        let s_caps = std::mem::take(&mut self.caps);
        let s_root = std::mem::replace(&mut self.root, body_id);
        let s_cix = std::mem::take(&mut self.closure_ix);
        let s_hint = self.closure_hint.take();
        let s_seg = self.seg_self.take();
        let s_lb = std::mem::take(&mut self.loop_bounds);
        let s_pb = self.pending_bound.take();
        let s_pp = self.pending_pair.take();
        let s_checked = std::mem::replace(&mut self.checked, (None, vec![]));
        let s_hoisted = std::mem::take(&mut self.hoisted);
        let s_fmt = std::mem::replace(&mut self.in_fmt_args, 0);
        let s_owned = std::mem::take(&mut self.owned);
        let s_temps = std::mem::take(&mut self.temps);
        let s_ob = std::mem::replace(&mut self.owned_binds, 0);
        let s_scrut = self.last_scrut.take();
        let s_margs = std::mem::replace(&mut self.margs, margs);
        let s_texts = std::mem::replace(&mut self.texts, texts);
        let s_iret = std::mem::replace(&mut self.inline_ret, Some(cont));
        self.inline_depth += 1;
        let r = self.inline_body(f, sig, if sret { &args[1..] } else { args });
        self.inline_depth -= 1;
        // 戻す
        self.scopes = s_scopes;
        self.loops = s_loops;
        self.pending_label = s_label;
        self.ret = s_ret;
        self.is_main = s_main;
        self.user_code = s_user;
        self.line_base = s_lbase;
        self.overflow_checks = s_ovf;
        self.index_panic_block = s_ipb;
        self.caller_loc = s_caller;
        self.self_sid = s_self;
        self.sret = s_sret;
        self.lets = s_lets;
        self.addr_taken = s_taken;
        self.env = s_env;
        self.caps = s_caps;
        self.root = s_root;
        self.closure_ix = s_cix;
        self.closure_hint = s_hint;
        self.seg_self = s_seg;
        self.loop_bounds = s_lb;
        self.pending_bound = s_pb;
        self.pending_pair = s_pp;
        self.checked = s_checked;
        self.hoisted = s_hoisted;
        self.in_fmt_args = s_fmt;
        self.owned = s_owned;
        self.temps = s_temps;
        self.owned_binds = s_ob;
        self.last_scrut = s_scrut;
        self.margs = s_margs;
        self.texts = s_texts;
        self.inline_ret = s_iret;
        r?;
        self.b.switch_to_block(cont);
        Ok(Some(rv))
    }

    /// 呼び先の本体 (body と同じだが、引数は与えた値)
    fn inline_body(&mut self, f: &ItemFn, sig: &FnSig, args: &[Value]) -> R<()> {
        let mut params = args.iter().copied();
        let mut names: Vec<String> = vec![];
        if f.sig.recv.is_some() {
            names.push("self".to_string());
        }
        for (p, _) in &f.sig.params {
            names.push(pat_name(p)?);
        }
        for (name, &ty) in names.into_iter().zip(&sig.params) {
            if !ty.is_agg() && clif_ty(ty).is_none() {
                self.bind(name, ty, None);
                continue;
            }
            let Some(v) = params.next() else {
                return Err("internal: parameter count mismatch (inline)".into());
            };
            if ty.is_agg() {
                // (呼ぶ側がコピー済み: そのメモリを使い、本体の終わりで drop する)
                self.scopes.last_mut().unwrap().push((name, Loc::Ptr(v), ty));
                self.own_local(Loc::Ptr(v), ty);
            } else {
                self.bind(name, ty, Some(v));
            }
        }
        let (v, t) = self.block(&f.block, Some(self.ret))?;
        if t == Ty::Never {
            self.b.ins().trap(TrapCode::unwrap_user(1));
            return Ok(());
        }
        self.same(SP, t, self.ret)?;
        self.drop_for_exit(0, 0);
        self.emit_return(v);
        Ok(())
    }
}
