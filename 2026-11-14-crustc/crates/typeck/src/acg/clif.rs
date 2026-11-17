//! Cranelift の FunctionBuilder / InstBuilder と同じ形の API を、命令を記録するだけの crustc_ir::Recorder の上に作る薄い層
//! (機械語にするのはバックエンド: codegen の lower が記録を minclif / Cranelift で組み立て直す)。
//! codegen.rs (Cranelift 版) のコードをほぼそのまま移せるようにするためのもの。
//! 型による命令の振り分け (i128 / 浮動小数点) もここでする。

pub use crate::ir::{Block, FloatCC, IntCC, StackSlot, Type, Value, Variable};
use crate::ir::{FuncRef as MFuncRef, Recorder as M};

pub mod types {
    use crate::ir::Type;
    pub const I8: Type = Type::I8;
    pub const I16: Type = Type::I16;
    pub const I32: Type = Type::I32;
    pub const I64: Type = Type::I64;
    pub const I128: Type = Type::I128;
    pub const F32: Type = Type::F32;
    pub const F64: Type = Type::F64;
}

/// メモリアクセスの印 (minclif では意味を持たない)
#[derive(Clone, Copy, Default)]
pub struct MemFlagsData;
impl MemFlagsData {
    pub fn trusted() -> Self {
        MemFlagsData
    }
    pub fn new() -> Self {
        MemFlagsData
    }
    pub fn set_notrap(&mut self) {}
}

#[derive(Clone, Copy)]
pub struct TrapCode(pub u8);
impl TrapCode {
    pub fn unwrap_user(c: u8) -> TrapCode {
        TrapCode(c)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AbiParam {
    pub value_type: Type,
}
impl AbiParam {
    pub fn new(t: Type) -> AbiParam {
        AbiParam { value_type: t }
    }
    pub fn sext(self) -> AbiParam {
        self
    }
    pub fn uext(self) -> AbiParam {
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signature {
    pub params: Vec<AbiParam>,
    pub returns: Vec<AbiParam>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FuncRef(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SigRef(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Inst(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlobalValue(pub u32);

pub struct FunctionBuilder {
    pub m: M,
    funcs: Vec<(MFuncRef, String, Vec<Type>)>,
    sigs: Vec<Vec<Type>>,
    results: Vec<Vec<Value>>,
    gvs: Vec<String>,
    /// iconst で作った値とその定数
    consts: crate::fxhash::FxMap<u32, i64>,
}

impl FunctionBuilder {
    pub fn new(sig: &Signature) -> FunctionBuilder {
        let ps = sig.params.iter().map(|p| p.value_type).collect();
        let rs = sig.returns.iter().map(|p| p.value_type).collect();
        FunctionBuilder { m: M::new(ps, rs), funcs: vec![], sigs: vec![], results: vec![], gvs: vec![], consts: crate::fxhash::FxMap::default() }
    }
    pub fn ins(&mut self) -> Ins<'_> {
        Ins(self)
    }
    pub fn func_params(&self) -> Vec<Value> {
        self.m.func_params()
    }
    pub fn value_type(&self, v: Value) -> Type {
        self.m.value_type(v)
    }
    pub fn create_block(&mut self) -> Block {
        self.m.create_block()
    }
    pub fn append_block_param(&mut self, b: Block, t: Type) -> Value {
        self.m.append_block_param(b, t)
    }
    pub fn block_params(&self, b: Block) -> Vec<Value> {
        self.m.block_params(b)
    }
    pub fn switch_to_block(&mut self, b: Block) {
        self.m.switch_to_block(b)
    }
    pub fn current_block(&self) -> Option<Block> {
        self.m.current_block()
    }
    pub fn seal_block(&mut self, _: Block) {}
    pub fn seal_all_blocks(&mut self) {}
    pub fn set_cold_block(&mut self, _: Block) {}
    pub fn declare_var(&mut self, t: Type) -> Variable {
        self.m.declare_var(t)
    }
    pub fn def_var(&mut self, var: Variable, v: Value) {
        // (i128 の変数は 16 byte: minclif の変数は 8 byte なので値をそのまま使う)
        self.m.def_var(var, v)
    }
    pub fn use_var(&mut self, var: Variable) -> Value {
        self.m.use_var(var)
    }
    /// (サイズ, アラインメント)
    pub fn create_stack_slot(&mut self, size: u32, align: u32) -> StackSlot {
        self.m.create_stack_slot(size, align)
    }
    /// 名前と戻り値の型で関数を登録する
    pub fn import_function(&mut self, name: &str, rets: Vec<Type>) -> FuncRef {
        if let Some(i) = self.funcs.iter().position(|f| f.1 == name && f.2 == rets) {
            return FuncRef(i as u32);
        }
        let r = self.m.import_function(name);
        self.funcs.push((r, name.to_string(), rets));
        FuncRef(self.funcs.len() as u32 - 1)
    }
    pub fn import_signature(&mut self, s: Signature) -> SigRef {
        self.sigs.push(s.returns.iter().map(|p| p.value_type).collect());
        SigRef(self.sigs.len() as u32 - 1)
    }
    pub fn symbol(&mut self, name: &str) -> GlobalValue {
        if let Some(i) = self.gvs.iter().position(|g| g == name) {
            return GlobalValue(i as u32);
        }
        self.gvs.push(name.to_string());
        GlobalValue(self.gvs.len() as u32 - 1)
    }
    /// 値が iconst の結果ならその定数
    pub fn iconst_value(&self, v: Value) -> Option<i64> {
        self.consts.get(&v.0).copied()
    }
    pub fn inst_results(&self, i: Inst) -> &[Value] {
        &self.results[i.0 as usize]
    }
    pub fn set_zero(&mut self, z: &[StackSlot]) {
        self.m.set_zero(z)
    }
    fn push_results(&mut self, v: Vec<Value>) -> Inst {
        self.results.push(v);
        Inst(self.results.len() as u32 - 1)
    }
}

/// 1 命令分の組み立て (Cranelift と同じく、使うたびに `b.ins()` で作る)
pub struct Ins<'a>(&'a mut FunctionBuilder);

fn is128(b: &M, v: Value) -> bool {
    b.value_type(v) == Type::I128
}

impl Ins<'_> {
    fn m(&mut self) -> &mut M {
        &mut self.0.m
    }
    pub fn iconst(self, t: Type, x: i64) -> Value {
        let m = &mut self.0.m;
        if t == Type::I128 {
            let lo = m.iconst(Type::I64, x);
            let hi = m.iconst(Type::I64, 0);
            return m.iconcat(lo, hi);
        }
        // (幅で切る: 0 拡張で持つ)
        let x = match t {
            Type::I8 => x as u8 as i64,
            Type::I16 => x as u16 as i64,
            Type::I32 => x as u32 as i64,
            _ => x,
        };
        let v = m.iconst(t, x);
        self.0.consts.insert(v.0, x);
        v
    }
    pub fn f64const(mut self, x: f64) -> Value {
        self.m().f64const(x)
    }
    pub fn f32const(mut self, x: f32) -> Value {
        self.m().f32const(x)
    }
    pub fn load(mut self, t: Type, _: MemFlagsData, a: Value, off: i32) -> Value {
        self.m().load(t, a, off)
    }
    pub fn store(mut self, _: MemFlagsData, v: Value, a: Value, off: i32) {
        self.m().store(v, a, off)
    }
    pub fn stack_addr(mut self, _: Type, s: StackSlot, off: i32) -> Value {
        self.m().stack_addr(s, off)
    }
    pub fn stack_load(mut self, t: Type, s: StackSlot, off: i32) -> Value {
        let m = self.m();
        let a = m.stack_addr(s, 0);
        m.load(t, a, off)
    }
    pub fn stack_store(mut self, v: Value, s: StackSlot, off: i32) {
        let m = self.m();
        let a = m.stack_addr(s, 0);
        m.store(v, a, off)
    }
    pub fn jump(mut self, b: Block, args: &[Value]) {
        self.m().jump(b, args)
    }
    pub fn brif(mut self, c: Value, a: Block, aa: &[Value], b: Block, ba: &[Value]) {
        self.m().brif(c, a, aa, b, ba)
    }
    pub fn return_(mut self, vs: &[Value]) {
        self.m().return_(vs)
    }
    pub fn trap(mut self, c: TrapCode) {
        self.m().trap_code(c.0)
    }
    pub fn trapz(mut self, v: Value, c: TrapCode) {
        self.m().trapz_code(v, c.0)
    }
    pub fn trapnz(mut self, v: Value, c: TrapCode) {
        self.m().trapnz_code(v, c.0)
    }
    pub fn call(self, f: FuncRef, args: &[Value]) -> Inst {
        let (r, _, rets) = self.0.funcs[f.0 as usize].clone();
        let out = self.0.m.call(r, args, &rets);
        self.0.push_results(out)
    }
    pub fn call_indirect(self, s: SigRef, p: Value, args: &[Value]) -> Inst {
        let rets = self.0.sigs[s.0 as usize].clone();
        let out = self.0.m.call_indirect(p, args, &rets);
        self.0.push_results(out)
    }
    pub fn symbol_value(self, _: Type, g: GlobalValue) -> Value {
        let n = self.0.gvs[g.0 as usize].clone();
        self.0.m.symbol_addr(&n)
    }
    pub fn func_addr(self, _: Type, f: FuncRef) -> Value {
        let n = self.0.funcs[f.0 as usize].1.clone();
        self.0.m.symbol_addr(&n)
    }

    // --- 整数 ---------------------------------------------------------
    pub fn iadd(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.iadd128(a, b) } else if m.value_type(a).is_float() { m.fadd(a, b) } else { m.iadd(a, b) }
    }
    pub fn isub(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.isub128(a, b) } else { m.isub(a, b) }
    }
    pub fn imul(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.imul128(a, b) } else { m.imul(a, b) }
    }
    pub fn udiv(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.udiv128(a, b) } else { m.udiv(a, b) }
    }
    pub fn sdiv(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.sdiv128(a, b) } else { m.sdiv(a, b) }
    }
    pub fn urem(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.urem128(a, b) } else { m.urem(a, b) }
    }
    pub fn srem(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.srem128(a, b) } else { m.srem(a, b) }
    }
    pub fn band(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.band128(a, b) } else { m.band(a, b) }
    }
    pub fn bor(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.bor128(a, b) } else { m.bor(a, b) }
    }
    pub fn bxor(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.bxor128(a, b) } else { m.bxor(a, b) }
    }
    pub fn ishl(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let b = if is128(m, b) { m.reduce128(Type::I64, b) } else { b };
            m.ishl128(a, b)
        } else {
            let b = if is128(m, b) { m.reduce128(Type::I64, b) } else { b };
            m.ishl(a, b)
        }
    }
    pub fn ushr(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        let b = if is128(m, b) { m.reduce128(Type::I64, b) } else { b };
        if is128(m, a) { m.ushr128(a, b) } else { m.ushr(a, b) }
    }
    pub fn sshr(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        let b = if is128(m, b) { m.reduce128(Type::I64, b) } else { b };
        if is128(m, a) { m.sshr128(a, b) } else { m.sshr(a, b) }
    }
    pub fn rotl(mut self, a: Value, b: Value) -> Value {
        self.m().rotl(a, b)
    }
    pub fn rotr(mut self, a: Value, b: Value) -> Value {
        self.m().rotr(a, b)
    }
    pub fn ineg(self, a: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let lo = m.iconst(Type::I64, 0);
            let hi = m.iconst(Type::I64, 0);
            let z = m.iconcat(lo, hi);
            m.isub128(z, a)
        } else if m.value_type(a).is_float() {
            m.fneg(a)
        } else {
            m.ineg(a)
        }
    }
    pub fn bnot(self, a: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let (lo, hi) = m.isplit(a);
            let (lo, hi) = (m.bnot(lo), m.bnot(hi));
            m.iconcat(lo, hi)
        } else {
            m.bnot(a)
        }
    }
    fn imm(self, a: Value, x: i64) -> (Self, Value) {
        let t = self.0.m.value_type(a);
        let k = Ins(&mut *self.0).iconst(t, x);
        (self, k)
    }
    pub fn iadd_imm(self, a: Value, x: i64) -> Value {
        let (s, k) = self.imm(a, x);
        s.iadd(a, k)
    }
    pub fn iadd_imm_s(self, a: Value, x: i64) -> Value {
        self.iadd_imm(a, x)
    }
    pub fn imul_imm_s(self, a: Value, x: i64) -> Value {
        let (s, k) = self.imm(a, x);
        s.imul(a, k)
    }
    pub fn band_imm_u(self, a: Value, x: i64) -> Value {
        let (s, k) = self.imm(a, x);
        s.band(a, k)
    }
    pub fn bxor_imm_u(self, a: Value, x: i64) -> Value {
        let (s, k) = self.imm(a, x);
        s.bxor(a, k)
    }
    pub fn ushr_imm_u(self, a: Value, x: i64) -> Value {
        let k = Ins(&mut *self.0).iconst(Type::I64, x);
        self.ushr(a, k)
    }
    pub fn sshr_imm_s(self, a: Value, x: i64) -> Value {
        let k = Ins(&mut *self.0).iconst(Type::I64, x);
        self.sshr(a, k)
    }
    pub fn icmp(self, cc: IntCC, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.icmp128(cc, a, b) } else { m.icmp(cc, a, b) }
    }
    pub fn icmp_imm(self, cc: IntCC, a: Value, x: i64) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            // (64 bit の即値を符号拡張して比べる)
            let lo = m.iconst(Type::I64, x);
            let hi = m.iconst(Type::I64, if x < 0 { -1 } else { 0 });
            let k = m.iconcat(lo, hi);
            m.icmp128(cc, a, k)
        } else {
            let (s, k) = self.imm(a, x);
            s.icmp(cc, a, k)
        }
    }
    pub fn icmp_imm_u(self, cc: IntCC, a: Value, x: i64) -> Value {
        self.icmp_imm(cc, a, x)
    }
    pub fn icmp_imm_s(self, cc: IntCC, a: Value, x: i64) -> Value {
        self.icmp_imm(cc, a, x)
    }
    pub fn select(self, c: Value, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let (al, ah) = m.isplit(a);
            let (bl, bh) = m.isplit(b);
            let lo = m.select(c, al, bl);
            let hi = m.select(c, ah, bh);
            m.iconcat(lo, hi)
        } else {
            m.select(c, a, b)
        }
    }
    pub fn uextend(self, t: Type, a: Value) -> Value {
        let m = &mut self.0.m;
        if t == Type::I128 {
            let a = if m.value_type(a) == Type::I64 { a } else { m.uextend(Type::I64, a) };
            m.extend128(a, false)
        } else {
            m.uextend(t, a)
        }
    }
    pub fn sextend(self, t: Type, a: Value) -> Value {
        let m = &mut self.0.m;
        if t == Type::I128 {
            let a = if m.value_type(a) == Type::I64 { a } else { m.sextend(Type::I64, a) };
            m.extend128(a, true)
        } else {
            m.sextend(t, a)
        }
    }
    pub fn ireduce(self, t: Type, a: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) { m.reduce128(t, a) } else { m.ireduce(t, a) }
    }
    pub fn iconcat(mut self, lo: Value, hi: Value) -> Value {
        self.m().iconcat(lo, hi)
    }
    pub fn isplit(mut self, v: Value) -> (Value, Value) {
        self.m().isplit(v)
    }
    pub fn uadd_overflow(self, a: Value, b: Value) -> (Value, Value) {
        let m = &mut self.0.m;
        if is128(m, a) {
            let r = m.iadd128(a, b);
            let of = m.icmp128(IntCC::UnsignedLessThan, r, a);
            return (r, of);
        }
        m.uadd_overflow(a, b)
    }
    pub fn usub_overflow(self, a: Value, b: Value) -> (Value, Value) {
        let m = &mut self.0.m;
        if is128(m, a) {
            let r = m.isub128(a, b);
            let of = m.icmp128(IntCC::UnsignedLessThan, a, b);
            return (r, of);
        }
        m.usub_overflow(a, b)
    }
    pub fn sadd_overflow(self, a: Value, b: Value) -> (Value, Value) {
        let m = &mut self.0.m;
        if is128(m, a) {
            let r = m.iadd128(a, b);
            let x = m.bxor128(a, r);
            let y = m.bxor128(b, r);
            let w = m.band128(x, y);
            let lo = m.iconst(Type::I64, 0);
            let z = m.iconcat(lo, lo);
            let of = m.icmp128(IntCC::SignedLessThan, w, z);
            return (r, of);
        }
        m.sadd_overflow(a, b)
    }
    pub fn ssub_overflow(self, a: Value, b: Value) -> (Value, Value) {
        let m = &mut self.0.m;
        if is128(m, a) {
            let r = m.isub128(a, b);
            let x = m.bxor128(a, b);
            let y = m.bxor128(a, r);
            let w = m.band128(x, y);
            let lo = m.iconst(Type::I64, 0);
            let z = m.iconcat(lo, lo);
            let of = m.icmp128(IntCC::SignedLessThan, w, z);
            return (r, of);
        }
        m.ssub_overflow(a, b)
    }
    pub fn umul_overflow(mut self, a: Value, b: Value) -> (Value, Value) {
        self.m().umul_overflow(a, b)
    }
    pub fn smul_overflow(mut self, a: Value, b: Value) -> (Value, Value) {
        self.m().smul_overflow(a, b)
    }
    pub fn umulhi(mut self, a: Value, b: Value) -> Value {
        self.m().umulhi(a, b)
    }
    pub fn clz(self, a: Value) -> Value {
        let m = &mut self.0.m;
        if !is128(m, a) {
            return m.clz(a);
        }
        // (上位が 0 なら 64 + 下位の clz)
        let (lo, hi) = m.isplit(a);
        let zh = m.icmp_imm(IntCC::Equal, hi, 0);
        let cl = m.clz(lo);
        let k = m.iconst(Type::I64, 64);
        let cl = m.iadd(cl, k);
        let ch = m.clz(hi);
        let r = m.select(zh, cl, ch);
        m.extend128(r, false)
    }
    pub fn ctz(self, a: Value) -> Value {
        let m = &mut self.0.m;
        if !is128(m, a) {
            return m.ctz(a);
        }
        let (lo, hi) = m.isplit(a);
        let zl = m.icmp_imm(IntCC::Equal, lo, 0);
        let th = m.ctz(hi);
        let k = m.iconst(Type::I64, 64);
        let th = m.iadd(th, k);
        let tl = m.ctz(lo);
        let r = m.select(zl, th, tl);
        m.extend128(r, false)
    }
    pub fn popcnt(self, a: Value) -> Value {
        let m = &mut self.0.m;
        if !is128(m, a) {
            return m.popcnt(a);
        }
        let (lo, hi) = m.isplit(a);
        let pl = m.popcnt(lo);
        let ph = m.popcnt(hi);
        let r = m.iadd(pl, ph);
        m.extend128(r, false)
    }
    pub fn bswap(mut self, a: Value) -> Value {
        self.m().bswap(a)
    }
    pub fn smin(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let c = m.icmp128(IntCC::SignedLessThan, a, b);
            return Ins(&mut *self.0).select(c, a, b);
        }
        m.smin(a, b)
    }
    pub fn umin(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let c = m.icmp128(IntCC::UnsignedLessThan, a, b);
            return Ins(&mut *self.0).select(c, a, b);
        }
        m.umin(a, b)
    }
    pub fn smax(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let c = m.icmp128(IntCC::SignedGreaterThan, a, b);
            return Ins(&mut *self.0).select(c, a, b);
        }
        m.smax(a, b)
    }
    pub fn umax(self, a: Value, b: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            let c = m.icmp128(IntCC::UnsignedGreaterThan, a, b);
            return Ins(&mut *self.0).select(c, a, b);
        }
        m.umax(a, b)
    }
    pub fn iabs(self, a: Value) -> Value {
        let m = &mut self.0.m;
        if is128(m, a) {
            // (負なら 0 - a)
            let lo = m.iconst(Type::I64, 0);
            let z = m.iconcat(lo, lo);
            let neg = m.icmp128(IntCC::SignedLessThan, a, z);
            let na = m.isub128(z, a);
            return Ins(&mut *self.0).select(neg, na, a);
        }
        m.iabs(a)
    }

    // --- 浮動小数点 ----------------------------------------------------
    pub fn fadd(mut self, a: Value, b: Value) -> Value {
        self.m().fadd(a, b)
    }
    pub fn fsub(mut self, a: Value, b: Value) -> Value {
        self.m().fsub(a, b)
    }
    pub fn fmul(mut self, a: Value, b: Value) -> Value {
        self.m().fmul(a, b)
    }
    pub fn fdiv(mut self, a: Value, b: Value) -> Value {
        self.m().fdiv(a, b)
    }
    pub fn sqrt(mut self, a: Value) -> Value {
        self.m().sqrt(a)
    }
    pub fn floor(mut self, a: Value) -> Value {
        self.m().floor(a)
    }
    pub fn ceil(mut self, a: Value) -> Value {
        self.m().ceil(a)
    }
    pub fn trunc(mut self, a: Value) -> Value {
        self.m().trunc(a)
    }
    pub fn fneg(mut self, a: Value) -> Value {
        self.m().fneg(a)
    }
    pub fn fabs(mut self, a: Value) -> Value {
        self.m().fabs(a)
    }
    pub fn fcopysign(mut self, a: Value, b: Value) -> Value {
        self.m().fcopysign(a, b)
    }
    pub fn fma(mut self, a: Value, b: Value, c: Value) -> Value {
        self.m().fma(a, b, c)
    }
    pub fn bitcast(mut self, t: Type, _: MemFlagsData, a: Value) -> Value {
        self.m().bitcast(t, a)
    }
    pub fn fcmp(mut self, cc: FloatCC, a: Value, b: Value) -> Value {
        self.m().fcmp(cc, a, b)
    }
    pub fn fpromote(mut self, _: Type, a: Value) -> Value {
        self.m().fpromote(a)
    }
    pub fn fdemote(mut self, _: Type, a: Value) -> Value {
        self.m().fdemote(a)
    }
    pub fn fcvt_from_sint(self, t: Type, a: Value) -> Value {
        let m = &mut self.0.m;
        let a = match m.value_type(a) {
            Type::I64 => a,
            Type::I128 => m.reduce128(Type::I64, a),
            _ => m.sextend(Type::I64, a),
        };
        m.fcvt_from_sint(t, a)
    }
    pub fn fcvt_from_uint(self, t: Type, a: Value) -> Value {
        let m = &mut self.0.m;
        let a = match m.value_type(a) {
            Type::I64 => a,
            Type::I128 => m.reduce128(Type::I64, a),
            _ => m.uextend(Type::I64, a),
        };
        m.fcvt_from_uint(t, a)
    }
    pub fn fcvt_to_sint_sat(mut self, t: Type, a: Value) -> Value {
        self.m().fcvt_to_sint_sat(t, a)
    }
    pub fn fcvt_to_uint_sat(mut self, t: Type, a: Value) -> Value {
        self.m().fcvt_to_uint_sat(t, a)
    }
}
