//! driver2 の Cranelift バックエンド: minclif の FunctionBuilder が記録した命令の列 (acg が呼んだ順) を、
//! Cranelift の IR に組み立て直してコンパイルする。acg・キャッシュ・.o の書き出しは minclif と同じものを使うので、
//! 「同じフロントエンド・同じ命令の列」でバックエンドだけを比べられる (minclif と Cranelift)。
//!
//! 返すのは minclif と同じ形: (機械語, 再配置 (名前・ELF の種類), trap の場所と番号)。
//! crustc 自身ではコンパイルできない (cranelift を使う) ので、セルフホストには入れない。

use std::collections::HashMap;

use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::{FloatCC as CFloatCC, IntCC as CIntCC};
use cranelift_codegen::ir::{
    self, AbiParam, ExtFuncData, ExternalName, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind, TrapCode,
    UserExternalName, UserFuncName, types,
};
use cranelift_codegen::isa::{CallConv, OwnedTargetIsa};
use cranelift_codegen::{Context, RelocTarget, binemit::Reloc as CReloc, settings, settings::Configurable};
use cranelift_frontend::{FunctionBuilder as CBuilder, FunctionBuilderContext, Variable};

use minclif::{FloatCC, FunctionBuilder, IntCC, Op, Reloc, StackSlot, Type};

/// Cranelift の ISA (opt: -O なら speed、fast_ra: single_pass のレジスタ割り当て)
pub fn make_isa(opt: bool, fast_ra: bool) -> OwnedTargetIsa {
    let mut f = settings::builder();
    f.set("opt_level", if opt { "speed" } else { "none" }).unwrap();
    f.set("regalloc_algorithm", if fast_ra { "single_pass" } else { "backtracking" }).unwrap();
    f.set("is_pic", "true").unwrap();
    f.set("unwind_info", "false").unwrap();
    f.set("enable_llvm_abi_extensions", "true").unwrap();
    cranelift_native::builder().unwrap().finish(settings::Flags::new(f)).unwrap()
}

thread_local! {
    static ISA: std::cell::RefCell<Option<(bool, bool, OwnedTargetIsa)>> = const { std::cell::RefCell::new(None) };
}

fn ty(t: Type) -> ir::Type {
    match t {
        Type::I8 => types::I8,
        Type::I16 => types::I16,
        Type::I32 => types::I32,
        Type::I64 => types::I64,
        Type::I128 => types::I128,
        Type::F32 => types::F32,
        Type::F64 => types::F64,
    }
}

fn icc(c: IntCC) -> CIntCC {
    match c {
        IntCC::Equal => CIntCC::Equal,
        IntCC::NotEqual => CIntCC::NotEqual,
        IntCC::SignedLessThan => CIntCC::SignedLessThan,
        IntCC::SignedLessThanOrEqual => CIntCC::SignedLessThanOrEqual,
        IntCC::SignedGreaterThan => CIntCC::SignedGreaterThan,
        IntCC::SignedGreaterThanOrEqual => CIntCC::SignedGreaterThanOrEqual,
        IntCC::UnsignedLessThan => CIntCC::UnsignedLessThan,
        IntCC::UnsignedLessThanOrEqual => CIntCC::UnsignedLessThanOrEqual,
        IntCC::UnsignedGreaterThan => CIntCC::UnsignedGreaterThan,
        IntCC::UnsignedGreaterThanOrEqual => CIntCC::UnsignedGreaterThanOrEqual,
    }
}

fn fcc(c: FloatCC) -> CFloatCC {
    match c {
        FloatCC::Equal => CFloatCC::Equal,
        FloatCC::NotEqual => CFloatCC::NotEqual,
        FloatCC::LessThan => CFloatCC::LessThan,
        FloatCC::LessThanOrEqual => CFloatCC::LessThanOrEqual,
        FloatCC::GreaterThan => CFloatCC::GreaterThan,
        FloatCC::GreaterThanOrEqual => CFloatCC::GreaterThanOrEqual,
        FloatCC::Unordered => CFloatCC::Unordered,
    }
}

/// minclif の記録から機械語を作る。zero: 入口で 0 にするスロット
pub fn compile(fb: &FunctionBuilder, zero: &[StackSlot], opt: bool) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String> {
    let fast_ra = std::env::var_os("CRUSTC_CLIF_FAST_RA").is_some();
    ISA.with(|cell| {
        let mut c = cell.borrow_mut();
        if !matches!(&*c, Some((o, f, _)) if *o == opt && *f == fast_ra) {
            *c = Some((opt, fast_ra, make_isa(opt, fast_ra)));
        }
        let isa = &c.as_ref().unwrap().2;
        build(fb, zero, &**isa)
    })
}

fn build(fb: &FunctionBuilder, zero: &[StackSlot], isa: &dyn cranelift_codegen::isa::TargetIsa) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String> {
    let mut sig = Signature::new(CallConv::SystemV);
    for &p in &fb.params {
        sig.params.push(AbiParam::new(ty(p)));
    }
    for &r in &fb.rets {
        sig.returns.push(AbiParam::new(ty(r)));
    }
    let mut func = ir::Function::with_name_signature(UserFuncName::user(0, 0), sig);
    let mut fctx = FunctionBuilderContext::new();
    let mut b = CBuilder::new(&mut func, &mut fctx);
    // (名前の表: 再配置の番号 → 名前)
    let mut names: Vec<String> = vec![];
    let mut vals: HashMap<u32, ir::Value> = HashMap::new();
    let mut blocks: HashMap<u32, ir::Block> = HashMap::new();
    let mut vars: HashMap<u32, Variable> = HashMap::new();
    let mut slots: HashMap<u32, ir::StackSlot> = HashMap::new();
    // (minclif の関数の番号 → 名前。呼ぶときのシグネチャごとに Cranelift の FuncRef を作る)
    let mut fnames: HashMap<u32, String> = HashMap::new();
    let mut frefs: HashMap<(String, Vec<ir::Type>, Vec<ir::Type>), ir::FuncRef> = HashMap::new();
    let mut gvs: HashMap<String, ir::GlobalValue> = HashMap::new();
    // 入口: 引数の値 (minclif の値 0..) と、0 にするスロット。最初のブロックへ飛ぶ
    let entry = b.create_block();
    b.append_block_params_for_function_params(entry);
    b.switch_to_block(entry);
    let ps = b.block_params(entry).to_vec();
    for (i, p) in ps.iter().enumerate() {
        vals.insert(i as u32, *p);
    }
    // 定義と別の区間 (ブロックを続けて書いた範囲) で使う値は Variable にする: acg は値を支配していないブロックで
    // 使うことがある (minclif は値をスロットに置くので困らない)。SSA は cranelift_frontend が組み立てる
    let mut gvars: HashMap<u32, Variable> = HashMap::new();
    if let Some(tr) = fb.track() {
        let segs = &tr.segs;
        let seg_of = |op: u32| -> usize {
            let (mut lo, mut hi) = (0usize, segs.len());
            while lo + 1 < hi {
                let m = (lo + hi) / 2;
                if segs[m].start <= op {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            lo
        };
        let mut first: HashMap<u32, usize> = HashMap::new();
        let mut multi: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for &(id, op, _) in &tr.ev {
            if id >= 1 << 28 || id < fb.params.len() as u32 {
                continue;
            }
            let sg = seg_of(op);
            match first.get(&id) {
                None => {
                    first.insert(id, sg);
                }
                Some(&f) if f != sg => {
                    multi.insert(id);
                }
                _ => {}
            }
        }
        let mut ids: Vec<u32> = multi.into_iter().collect();
        ids.sort_unstable();
        for id in ids {
            let var = b.declare_var(ty(fb.value_type(minclif::Value(id))));
            gvars.insert(id, var);
        }
    }
    // スタックのスロットのフィールドを Variable にする (SROA: minclif と同じ判定。アドレスが load / store にしか
    // 使われないスロット)。Cranelift はスタックのスロットを SSA にしないので、struct の値がメモリを往復しなくなる
    let mut facc: HashMap<u32, Variable> = HashMap::new();
    if let (Some(tr), true) = (fb.track(), std::env::var_os("CRUSTC_CLIF_NO_SROA").is_none()) {
        let zs: Vec<u32> = zero.iter().map(|z| z.0).collect();
        let (prom, fields, ftys) = minclif::sroa(tr, fb.n_values(), fb.paddr(), fb.n_slots(), &zs);
        if !fields.is_empty() {
            let fv: Vec<Variable> = ftys
                .iter()
                .map(|&t| {
                    let var = b.declare_var(ty(t));
                    // (入口で 0: 読む前に書かない経路も決まった値にする)
                    let z = match t {
                        Type::F64 => b.ins().f64const(0.0),
                        Type::F32 => b.ins().f32const(0.0),
                        t => b.ins().iconst(ty(t), 0),
                    };
                    b.def_var(var, z);
                    var
                })
                .collect();
            for &(op, sl, off, _, _) in &tr.acc {
                if prom[sl as usize] {
                    if let Some(&f) = fields.get(&(sl, off)) {
                        facc.insert(op, fv[f as usize]);
                    }
                }
            }
        }
    }
    // (ブロックの引数: Variable にしたものはブロックの始まりで def_var する)
    let mut bparams: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut started: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut in_entry = true;
    macro_rules! v {
        ($x:expr) => {
            match gvars.get(&$x.0) {
                Some(&var) => b.use_var(var),
                None => *vals.get(&$x.0).ok_or_else(|| format!("clif_backend: unknown value v{}", $x.0))?,
            }
        };
    }
    macro_rules! vs {
        ($xs:expr) => {{
            let mut out: Vec<ir::Value> = vec![];
            for x in $xs.iter() {
                out.push(v!(x));
            }
            out
        }};
    }
    let name_of = |n: &str, names: &mut Vec<String>| -> u32 {
        match names.iter().position(|x| x == n) {
            Some(i) => i as u32,
            None => {
                names.push(n.to_string());
                names.len() as u32 - 1
            }
        }
    };
    for (i, op) in fb.ops.iter().enumerate() {
        let res = &fb.res[i];
        // (値を作る命令: Variable にした値はここで def_var)
        let makes_values = !matches!(
            op,
            Op::CreateBlock | Op::AppendBlockParam(..) | Op::SwitchToBlock(_) | Op::DeclareVar(_) | Op::CreateStackSlot(..) | Op::ImportFunction(_)
        );
        let set1 = |vals: &mut HashMap<u32, ir::Value>, v: ir::Value| {
            vals.insert(res[0], v);
        };
        match op {
            Op::CreateBlock => {
                let bl = b.create_block();
                blocks.insert(res[0], bl);
            }
            Op::AppendBlockParam(bk, t) => {
                let bl = blocks[&bk.0];
                let v = b.append_block_param(bl, ty(*t));
                set1(&mut vals, v);
                bparams.entry(bk.0).or_default().push(res[0]);
            }
            Op::SwitchToBlock(bk) => {
                let bl = blocks[&bk.0];
                if in_entry {
                    b.ins().jump(bl, &[]);
                    in_entry = false;
                }
                b.switch_to_block(bl);
                if started.insert(bk.0) {
                    for id in bparams.get(&bk.0).cloned().unwrap_or_default() {
                        if let Some(&var) = gvars.get(&id) {
                            b.def_var(var, vals[&id]);
                        }
                    }
                }
            }
            Op::DeclareVar(t) => {
                let var = b.declare_var(ty(*t));
                vars.insert(res[0], var);
            }
            Op::DefVar(var, x) => {
                let x = v!(x);
                b.def_var(vars[&var.0], x);
            }
            Op::UseVar(var) => {
                let x = b.use_var(vars[&var.0]);
                set1(&mut vals, x);
            }
            Op::CreateStackSlot(size, align) => {
                let ss = b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (*size).max(1), (*align).max(1).trailing_zeros() as u8));
                slots.insert(res[0], ss);
            }
            Op::ImportFunction(n) => {
                fnames.insert(res[0], n.clone());
            }
            Op::Iconst(t, x) => {
                let x = if *t == Type::I128 {
                    let lo = b.ins().iconst(types::I64, *x);
                    let hi = b.ins().iconst(types::I64, if *x < 0 { -1 } else { 0 });
                    b.ins().iconcat(lo, hi)
                } else {
                    b.ins().iconst(ty(*t), *x)
                };
                set1(&mut vals, x);
            }
            Op::Iadd(x, y) | Op::Iadd128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().iadd(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Isub(x, y) | Op::Isub128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().isub(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Imul(x, y) | Op::Imul128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().imul(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Band(x, y) | Op::Band128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().band(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Bor(x, y) | Op::Bor128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().bor(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Bxor(x, y) | Op::Bxor128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().bxor(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::IaddImm(x, k) => {
                let _t0 = v!(x);
                let r = b.ins().iadd_imm_s(_t0, *k);
                set1(&mut vals, r);
            }
            Op::Udiv(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().udiv(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Sdiv(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().sdiv(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Urem(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().urem(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Srem(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().srem(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Ishl(x, y) | Op::Ishl128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().ishl(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Ushr(x, y) | Op::Ushr128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().ushr(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Sshr(x, y) | Op::Sshr128(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().sshr(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Ineg(x) => {
                let _t0 = v!(x);
                let r = b.ins().ineg(_t0);
                set1(&mut vals, r);
            }
            Op::Bnot(x) => {
                let _t0 = v!(x);
                let r = b.ins().bnot(_t0);
                set1(&mut vals, r);
            }
            Op::Icmp(c, x, y) | Op::Icmp128(c, x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().icmp(icc(*c), _t0, _t1);
                set1(&mut vals, r);
            }
            Op::IcmpImm(c, x, k) => {
                let _t0 = v!(x);
                let r = b.ins().icmp_imm_s(icc(*c), _t0, *k);
                set1(&mut vals, r);
            }
            Op::Select(c, x, y) => {
                let _t0 = v!(c);
                let _t1 = v!(x);
                let _t2 = v!(y);
                let r = b.ins().select(_t0, _t1, _t2);
                set1(&mut vals, r);
            }
            Op::Uextend(t, x) | Op::Sextend(t, x) | Op::Ireduce(t, x) => {
                let xv = v!(x);
                let from = b.func.dfg.value_type(xv);
                let to = ty(*t);
                let r = if from == to {
                    xv
                } else if from.bits() > to.bits() {
                    b.ins().ireduce(to, xv)
                } else if matches!(op, Op::Sextend(..)) {
                    b.ins().sextend(to, xv)
                } else {
                    b.ins().uextend(to, xv)
                };
                set1(&mut vals, r);
            }
            Op::Extend128(x, signed) => {
                let _t0 = v!(x);
                let _t1 = v!(x);
                let r = if *signed { b.ins().sextend(types::I128, _t0) } else { b.ins().uextend(types::I128, _t1) };
                set1(&mut vals, r);
            }
            Op::Reduce128(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().ireduce(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::Load(_, _, _) if facc.contains_key(&(i as u32)) => {
                let r = b.use_var(facc[&(i as u32)]);
                set1(&mut vals, r);
            }
            Op::Store(x, _, _) if facc.contains_key(&(i as u32)) => {
                let x = v!(x);
                b.def_var(facc[&(i as u32)], x);
            }
            Op::Load(t, a, off) => {
                let _t0 = v!(a);
                let r = b.ins().load(ty(*t), MemFlagsData::trusted(), _t0, *off);
                set1(&mut vals, r);
            }
            Op::Store(x, a, off) => {
                let _t0 = v!(x);
                let _t1 = v!(a);
                b.ins().store(MemFlagsData::trusted(), _t0, _t1, *off);
            }
            Op::StackAddr(s, off) => {
                let r = b.ins().stack_addr(types::I64, slots[&s.0], *off);
                set1(&mut vals, r);
            }
            Op::Jump(bk, args) => {
                let a: Vec<ir::BlockArg> = vs!(args).into_iter().map(|x| x.into()).collect();
                b.ins().jump(blocks[&bk.0], &a);
            }
            Op::Brif(c, tb, ta, eb, ea) => {
                let cv = v!(c);
                let ta: Vec<ir::BlockArg> = vs!(ta).into_iter().map(|x| x.into()).collect();
                let ea: Vec<ir::BlockArg> = vs!(ea).into_iter().map(|x| x.into()).collect();
                b.ins().brif(cv, blocks[&tb.0], &ta, blocks[&eb.0], &ea);
            }
            Op::Return_(rs) => {
                let r: Vec<ir::Value> = vs!(rs);
                b.ins().return_(&r);
            }
            Op::Trap => {
                b.ins().trap(TrapCode::unwrap_user(1));
            }
            Op::TrapCode(c) => {
                b.ins().trap(TrapCode::unwrap_user(*c));
            }
            Op::Trapz(c) => {
                let _t0 = v!(c);
                b.ins().trapz(_t0, TrapCode::unwrap_user(1));
            }
            Op::Trapnz(c) => {
                let _t0 = v!(c);
                b.ins().trapnz(_t0, TrapCode::unwrap_user(1));
            }
            Op::TrapzCode(c, k) => {
                let _t0 = v!(c);
                b.ins().trapz(_t0, TrapCode::unwrap_user(*k));
            }
            Op::TrapnzCode(c, k) => {
                let _t0 = v!(c);
                b.ins().trapnz(_t0, TrapCode::unwrap_user(*k));
            }
            Op::Call(f, args, rets) => {
                let av: Vec<ir::Value> = vs!(args);
                let pt: Vec<ir::Type> = av.iter().map(|&x| b.func.dfg.value_type(x)).collect();
                let rt: Vec<ir::Type> = rets.iter().map(|&t| ty(t)).collect();
                let n = fnames.get(&f.0).cloned().unwrap_or_default();
                let key = (n.clone(), pt.clone(), rt.clone());
                let fr = match frefs.get(&key) {
                    Some(&fr) => fr,
                    None => {
                        let mut s = Signature::new(CallConv::SystemV);
                        s.params = pt.iter().map(|&t| AbiParam::new(t)).collect();
                        s.returns = rt.iter().map(|&t| AbiParam::new(t)).collect();
                        let sr = b.import_signature(s);
                        let idx = name_of(&n, &mut names);
                        let nm = b.func.declare_imported_user_function(UserExternalName::new(0, idx));
                        let fr = b.import_function(ExtFuncData { name: ExternalName::user(nm), signature: sr, colocated: false, patchable: false });
                        frefs.insert(key, fr);
                        fr
                    }
                };
                let inst = b.ins().call(fr, &av);
                let rs = b.inst_results(inst).to_vec();
                for (k, r) in rs.iter().enumerate() {
                    if let Some(&id) = res.get(k) {
                        vals.insert(id, *r);
                    }
                }
            }
            Op::CallIndirect(p, args, rets) => {
                let av: Vec<ir::Value> = vs!(args);
                let mut s = Signature::new(CallConv::SystemV);
                s.params = av.iter().map(|&x| AbiParam::new(b.func.dfg.value_type(x))).collect();
                s.returns = rets.iter().map(|&t| AbiParam::new(ty(t))).collect();
                let sr = b.import_signature(s);
                let pv = v!(p);
                let inst = b.ins().call_indirect(sr, pv, &av);
                let rs = b.inst_results(inst).to_vec();
                for (k, r) in rs.iter().enumerate() {
                    if let Some(&id) = res.get(k) {
                        vals.insert(id, *r);
                    }
                }
            }
            Op::SymbolAddr(n) => {
                let gv = match gvs.get(n) {
                    Some(&g) => g,
                    None => {
                        let idx = name_of(n, &mut names);
                        let nm = b.func.declare_imported_user_function(UserExternalName::new(0, idx));
                        let g = b.create_global_value(ir::GlobalValueData::Symbol {
                            name: ExternalName::user(nm),
                            offset: ir::immediates::Imm64::new(0),
                            colocated: false,
                            tls: false,
                        });
                        gvs.insert(n.clone(), g);
                        g
                    }
                };
                let r = b.ins().symbol_value(types::I64, gv);
                set1(&mut vals, r);
            }
            Op::F64const(x) => {
                let r = b.ins().f64const(*x);
                set1(&mut vals, r);
            }
            Op::F32const(x) => {
                let r = b.ins().f32const(*x);
                set1(&mut vals, r);
            }
            Op::Fadd(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fadd(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fsub(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fsub(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fmul(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fmul(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fdiv(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fdiv(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Sqrt(x) => {
                let _t0 = v!(x);
                let r = b.ins().sqrt(_t0);
                set1(&mut vals, r);
            }
            Op::Floor(x) => {
                let _t0 = v!(x);
                let r = b.ins().floor(_t0);
                set1(&mut vals, r);
            }
            Op::Ceil(x) => {
                let _t0 = v!(x);
                let r = b.ins().ceil(_t0);
                set1(&mut vals, r);
            }
            Op::Trunc(x) => {
                let _t0 = v!(x);
                let r = b.ins().trunc(_t0);
                set1(&mut vals, r);
            }
            Op::Fneg(x) => {
                let _t0 = v!(x);
                let r = b.ins().fneg(_t0);
                set1(&mut vals, r);
            }
            Op::Fabs(x) => {
                let _t0 = v!(x);
                let r = b.ins().fabs(_t0);
                set1(&mut vals, r);
            }
            Op::Fcopysign(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fcopysign(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fma(x, y, z) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let _t2 = v!(z);
                let r = b.ins().fma(_t0, _t1, _t2);
                set1(&mut vals, r);
            }
            Op::Bitcast(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().bitcast(ty(*t), MemFlagsData::new(), _t0);
                set1(&mut vals, r);
            }
            Op::Fcmp(c, x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().fcmp(fcc(*c), _t0, _t1);
                set1(&mut vals, r);
            }
            Op::Fpromote(x) => {
                let _t0 = v!(x);
                let r = b.ins().fpromote(types::F64, _t0);
                set1(&mut vals, r);
            }
            Op::Fdemote(x) => {
                let _t0 = v!(x);
                let r = b.ins().fdemote(types::F32, _t0);
                set1(&mut vals, r);
            }
            Op::FcvtFromSint(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_from_sint(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::FcvtFromUint(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_from_uint(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::FcvtToSintSat(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_to_sint_sat(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::FcvtToUintSat(t, x) => {
                let _t0 = v!(x);
                let r = b.ins().fcvt_to_uint_sat(ty(*t), _t0);
                set1(&mut vals, r);
            }
            Op::Iconcat(lo, hi) => {
                let _t0 = v!(lo);
                let _t1 = v!(hi);
                let r = b.ins().iconcat(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Isplit(x) => {
                let _t0 = v!(x);
                let (lo, hi) = b.ins().isplit(_t0);
                vals.insert(res[0], lo);
                vals.insert(res[1], hi);
            }
            Op::Udiv128(x, y) | Op::Sdiv128(x, y) | Op::Urem128(x, y) | Op::Srem128(x, y) => {
                // (i128 の割り算は libgcc: minclif と同じ)
                let n = match op {
                    Op::Udiv128(..) => "__udivti3",
                    Op::Sdiv128(..) => "__divti3",
                    Op::Urem128(..) => "__umodti3",
                    _ => "__modti3",
                };
                let mut s = Signature::new(CallConv::SystemV);
                s.params = vec![AbiParam::new(types::I128), AbiParam::new(types::I128)];
                s.returns = vec![AbiParam::new(types::I128)];
                let sr = b.import_signature(s);
                let idx = name_of(n, &mut names);
                let nm = b.func.declare_imported_user_function(UserExternalName::new(0, idx));
                let fr = b.import_function(ExtFuncData { name: ExternalName::user(nm), signature: sr, colocated: false, patchable: false });
                let (xv, yv) = (v!(x), v!(y));
                let inst = b.ins().call(fr, &[xv, yv]);
                let r = b.inst_results(inst)[0];
                set1(&mut vals, r);
            }
            Op::UaddOverflow(x, y) | Op::UsubOverflow(x, y) | Op::SaddOverflow(x, y) | Op::SsubOverflow(x, y) | Op::UmulOverflow(x, y) | Op::SmulOverflow(x, y) => {
                let (xv, yv) = (v!(x), v!(y));
                let (r, o) = match op {
                    Op::UaddOverflow(..) => b.ins().uadd_overflow(xv, yv),
                    Op::UsubOverflow(..) => b.ins().usub_overflow(xv, yv),
                    Op::SaddOverflow(..) => b.ins().sadd_overflow(xv, yv),
                    Op::SsubOverflow(..) => b.ins().ssub_overflow(xv, yv),
                    Op::UmulOverflow(..) => b.ins().umul_overflow(xv, yv),
                    _ => b.ins().smul_overflow(xv, yv),
                };
                vals.insert(res[0], r);
                vals.insert(res[1], o);
            }
            Op::Umulhi(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().umulhi(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Clz(x) => {
                let _t0 = v!(x);
                let r = b.ins().clz(_t0);
                set1(&mut vals, r);
            }
            Op::Ctz(x) => {
                let _t0 = v!(x);
                let r = b.ins().ctz(_t0);
                set1(&mut vals, r);
            }
            Op::Popcnt(x) => {
                let _t0 = v!(x);
                let r = b.ins().popcnt(_t0);
                set1(&mut vals, r);
            }
            Op::Bswap(x) => {
                let _t0 = v!(x);
                let r = b.ins().bswap(_t0);
                set1(&mut vals, r);
            }
            Op::Rotl(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().rotl(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Rotr(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().rotr(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Smin(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().smin(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Umin(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().umin(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Smax(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().smax(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Umax(x, y) => {
                let _t0 = v!(x);
                let _t1 = v!(y);
                let r = b.ins().umax(_t0, _t1);
                set1(&mut vals, r);
            }
            Op::Iabs(x) => {
                let _t0 = v!(x);
                let r = b.ins().iabs(_t0);
                set1(&mut vals, r);
            }
        }
        if makes_values {
            for id in res {
                if let (Some(&var), Some(&v)) = (gvars.get(id), vals.get(id)) {
                    b.def_var(var, v);
                }
            }
        }
    }
    b.seal_all_blocks();
    b.finalize(isa.frontend_config());
    // (入口で 0 にするスロット (drop フラグ): 途中で作られるので、組み立てた後で入口の先頭に書く)
    if !zero.is_empty() {
        use cranelift_codegen::cursor::{Cursor, FuncCursor};
        let mut pos = FuncCursor::new(&mut func).at_first_insertion_point(entry);
        for z in zero {
            if let Some(&ss) = slots.get(&z.0) {
                let k = pos.ins().iconst(types::I8, 0);
                pos.ins().stack_store(types::I64, k, ss, 0);
            }
        }
    }
    let mut ctx = Context::for_function(func);
    ctx.compile(isa, &mut ControlPlane::default()).map_err(|e| format!("cranelift: {:?}", e.inner))?;
    let code = ctx.compiled_code().unwrap();
    let mut relocs = vec![];
    for r in code.buffer.relocs() {
        let name = match r.target {
            RelocTarget::ExternalName(ExternalName::User(reff)) => {
                let n = &ctx.func.params.user_named_funcs()[reff];
                names.get(n.index as usize).cloned().unwrap_or_default()
            }
            ref t => return Err(format!("cranelift: unsupported reloc target {t:?}")),
        };
        let kind = match r.kind {
            CReloc::X86CallPLTRel4 | CReloc::X86CallPCRel4 => 4,
            CReloc::X86GOTPCRel4 => 9,
            CReloc::X86PCRel4 => 2,
            CReloc::Abs8 => 1,
            k => return Err(format!("cranelift: unsupported reloc kind {k:?}")),
        };
        relocs.push(Reloc { offset: r.offset, name, kind, addend: r.addend });
    }
    let traps: Vec<(u32, u8)> = code.buffer.traps().iter().map(|t| (t.offset, t.code.as_raw().get())).collect();
    Ok((code.buffer.data().to_vec(), relocs, traps))
}
