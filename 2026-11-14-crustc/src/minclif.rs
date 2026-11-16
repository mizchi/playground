//! Cranelift の代わりの最小のバックエンド (x86-64 / System V / ELF)。セルフホストのために Cranelift を置き換える土台。
//!
//! API は Cranelift の FunctionBuilder に寄せてある (ブロック・ブロック引数・変数・スタックスロット・命令)。
//! 生成は単純: 値はすべてフレームの 8 byte のスロットに置き、命令ごとに rax / rcx に読んで計算して書き戻す。
//! 整数の値は型の幅で 0 拡張した形でスロットに持つ (符号つきの比較・拡張のときだけ符号拡張する)。
//! 外部クレートに依存しないので crustc 自身でもコンパイルできる。tests/minclif.rs が cc でリンクして動かす。

/// 値の型
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Type {
    I8,
    I16,
    I32,
    I64,
}

impl Type {
    pub fn bytes(self) -> u32 {
        match self {
            Type::I8 => 1,
            Type::I16 => 2,
            Type::I32 => 4,
            Type::I64 => 8,
        }
    }
}

/// 整数の比較
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IntCC {
    Equal,
    NotEqual,
    SignedLessThan,
    SignedLessThanOrEqual,
    SignedGreaterThan,
    SignedGreaterThanOrEqual,
    UnsignedLessThan,
    UnsignedLessThanOrEqual,
    UnsignedGreaterThan,
    UnsignedGreaterThanOrEqual,
}

impl IntCC {
    fn signed(self) -> bool {
        matches!(
            self,
            IntCC::SignedLessThan | IntCC::SignedLessThanOrEqual | IntCC::SignedGreaterThan | IntCC::SignedGreaterThanOrEqual
        )
    }
    /// setcc の 2 byte 目
    fn setcc(self) -> u8 {
        match self {
            IntCC::Equal => 0x94,
            IntCC::NotEqual => 0x95,
            IntCC::SignedLessThan => 0x9C,
            IntCC::SignedLessThanOrEqual => 0x9E,
            IntCC::SignedGreaterThan => 0x9F,
            IntCC::SignedGreaterThanOrEqual => 0x9D,
            IntCC::UnsignedLessThan => 0x92,
            IntCC::UnsignedLessThanOrEqual => 0x96,
            IntCC::UnsignedGreaterThan => 0x97,
            IntCC::UnsignedGreaterThanOrEqual => 0x93,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Value(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Block(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Variable(pub u32);
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct StackSlot(pub u32);
/// 呼び出す関数 (外部の名前)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct FuncRef(pub u32);

/// コードの中の再配置 (call の rel32 など)
#[derive(Clone, Debug, PartialEq)]
pub struct Reloc {
    pub offset: u32,
    /// 参照する名前 (FuncRef の番号の名前)
    pub name: String,
    /// R_X86_64_PLT32 (4) / R_X86_64_PC32 (2)
    pub kind: u32,
    pub addend: i64,
}

/// 1 関数分の組み立て
pub struct FunctionBuilder {
    params: Vec<Type>,
    rets: Vec<Type>,
    /// 値のスロット (rbp からの負のオフセット) と型
    values: Vec<(i32, Type)>,
    vars: Vec<(i32, Type)>,
    slots: Vec<i32>,
    frame: i32,
    blocks: Vec<BlockData>,
    cur: Option<Block>,
    funcs: Vec<String>,
    code: Vec<u8>,
    relocs: Vec<Reloc>,
    /// 前方参照のジャンプ: (rel32 を書く位置, 飛び先)
    fixups: Vec<(usize, Block)>,
    entry_params: Vec<Value>,
}

struct BlockData {
    params: Vec<Value>,
    /// コードの位置 (切り替えたとき)
    offset: Option<usize>,
}

impl FunctionBuilder {
    pub fn new(params: Vec<Type>, rets: Vec<Type>) -> FunctionBuilder {
        let mut b = FunctionBuilder {
            params: params.clone(),
            rets,
            values: vec![],
            vars: vec![],
            slots: vec![],
            frame: 0,
            blocks: vec![],
            cur: None,
            funcs: vec![],
            code: vec![],
            relocs: vec![],
            fixups: vec![],
            entry_params: vec![],
        };
        // プロローグ: push rbp; mov rbp, rsp; sub rsp, <frame> (frame は最後に書き込む)
        b.code.extend_from_slice(&[0x55, 0x48, 0x89, 0xE5, 0x48, 0x81, 0xEC, 0, 0, 0, 0]);
        // 引数をスロットへ
        const ARG_STORE: [[u8; 3]; 6] = [[0x48, 0x89, 0xBD], [0x48, 0x89, 0xB5], [0x48, 0x89, 0x95], [0x48, 0x89, 0x8D], [0x4C, 0x89, 0x85], [0x4C, 0x89, 0x8D]];
        for (i, &t) in params.iter().enumerate() {
            let v = b.new_value(t);
            let off = b.values[v.0 as usize].0;
            if i < 6 {
                b.code.extend_from_slice(&ARG_STORE[i]);
                b.code.extend_from_slice(&off.to_le_bytes());
            } else {
                // 7 個目以降はスタック渡し: [rbp + 16 + 8*(i-6)]
                let src = 16 + 8 * (i as i32 - 6);
                b.code.extend_from_slice(&[0x48, 0x8B, 0x85]);
                b.code.extend_from_slice(&src.to_le_bytes());
                b.store_rax(v);
            }
            b.entry_params.push(v);
        }
        b
    }

    /// 関数の引数の値
    pub fn func_params(&self) -> Vec<Value> {
        self.entry_params.clone()
    }

    fn alloc(&mut self, size: u32, align: u32) -> i32 {
        let a = align.max(1) as i32;
        self.frame = (self.frame + size as i32 + a - 1) / a * a;
        -self.frame
    }

    fn new_value(&mut self, t: Type) -> Value {
        let off = self.alloc(8, 8);
        self.values.push((off, t));
        Value(self.values.len() as u32 - 1)
    }

    pub fn value_type(&self, v: Value) -> Type {
        self.values[v.0 as usize].1
    }

    pub fn create_block(&mut self) -> Block {
        self.blocks.push(BlockData { params: vec![], offset: None });
        Block(self.blocks.len() as u32 - 1)
    }

    pub fn append_block_param(&mut self, b: Block, t: Type) -> Value {
        let v = self.new_value(t);
        self.blocks[b.0 as usize].params.push(v);
        v
    }

    pub fn block_params(&self, b: Block) -> Vec<Value> {
        self.blocks[b.0 as usize].params.clone()
    }

    pub fn switch_to_block(&mut self, b: Block) {
        self.blocks[b.0 as usize].offset = Some(self.code.len());
        self.cur = Some(b);
    }

    pub fn current_block(&self) -> Option<Block> {
        self.cur
    }

    pub fn declare_var(&mut self, t: Type) -> Variable {
        let off = self.alloc(8, 8);
        self.vars.push((off, t));
        Variable(self.vars.len() as u32 - 1)
    }

    pub fn def_var(&mut self, var: Variable, v: Value) {
        self.load_rax(v);
        let off = self.vars[var.0 as usize].0;
        self.store_rax_at(off);
    }

    pub fn use_var(&mut self, var: Variable) -> Value {
        let (off, t) = self.vars[var.0 as usize];
        self.load_rax_at(off);
        let v = self.new_value(t);
        self.store_rax(v);
        v
    }

    pub fn create_stack_slot(&mut self, size: u32, align: u32) -> StackSlot {
        let off = self.alloc(size.max(1), align.max(1).min(16));
        self.slots.push(off);
        StackSlot(self.slots.len() as u32 - 1)
    }

    /// 呼ぶ関数の名前を登録する
    pub fn import_function(&mut self, name: &str) -> FuncRef {
        if let Some(i) = self.funcs.iter().position(|f| f == name) {
            return FuncRef(i as u32);
        }
        self.funcs.push(name.to_string());
        FuncRef(self.funcs.len() as u32 - 1)
    }

    // --- 命令の部品 ---------------------------------------------------

    fn emit(&mut self, b: &[u8]) {
        self.code.extend_from_slice(b);
    }

    fn emit_disp(&mut self, op: &[u8], d: i32) {
        self.code.extend_from_slice(op);
        self.code.extend_from_slice(&d.to_le_bytes());
    }

    fn load_rax_at(&mut self, off: i32) {
        self.emit_disp(&[0x48, 0x8B, 0x85], off);
    }

    fn store_rax_at(&mut self, off: i32) {
        self.emit_disp(&[0x48, 0x89, 0x85], off);
    }

    fn load_rax(&mut self, v: Value) {
        let off = self.values[v.0 as usize].0;
        self.load_rax_at(off);
    }

    fn load_rcx(&mut self, v: Value) {
        let off = self.values[v.0 as usize].0;
        self.emit_disp(&[0x48, 0x8B, 0x8D], off);
    }

    fn store_rax(&mut self, v: Value) {
        let off = self.values[v.0 as usize].0;
        self.store_rax_at(off);
    }

    /// rax を型の幅で 0 拡張する (I64 は何もしない)
    fn zext_rax(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xB6, 0xC0]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xB7, 0xC0]),
            Type::I32 => self.emit(&[0x89, 0xC0]),
            Type::I64 => {}
        }
    }

    /// rax を型の幅から符号拡張する
    fn sext_rax(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xBE, 0xC0]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xBF, 0xC0]),
            Type::I32 => self.emit(&[0x48, 0x63, 0xC0]),
            Type::I64 => {}
        }
    }

    fn sext_rcx(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xBE, 0xC9]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xBF, 0xC9]),
            Type::I32 => self.emit(&[0x48, 0x63, 0xC9]),
            Type::I64 => {}
        }
    }

    fn result(&mut self, t: Type) -> Value {
        self.zext_rax(t);
        let v = self.new_value(t);
        self.store_rax(v);
        v
    }

    // --- 命令 ----------------------------------------------------------

    pub fn iconst(&mut self, t: Type, x: i64) -> Value {
        self.emit(&[0x48, 0xB8]);
        self.emit(&x.to_le_bytes());
        self.result(t)
    }

    fn binop(&mut self, a: Value, b: Value, op: &[u8]) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.load_rcx(b);
        self.emit(op);
        self.result(t)
    }

    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x01, 0xC8])
    }
    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x29, 0xC8])
    }
    pub fn imul(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x0F, 0xAF, 0xC1])
    }
    pub fn band(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x21, 0xC8])
    }
    pub fn bor(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x09, 0xC8])
    }
    pub fn bxor(&mut self, a: Value, b: Value) -> Value {
        self.binop(a, b, &[0x48, 0x31, 0xC8])
    }

    pub fn iadd_imm(&mut self, a: Value, x: i64) -> Value {
        let t = self.value_type(a);
        let k = self.iconst(t, x);
        let _ = t;
        self.iadd(a, k)
    }

    /// 符号なし / 符号つきの割り算・余り (0 除算の検査は呼び出し側)
    fn divop(&mut self, a: Value, b: Value, signed: bool, rem: bool) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.load_rcx(b);
        if signed {
            self.sext_rax(t);
            self.sext_rcx(t);
            self.emit(&[0x48, 0x99]); // cqo
            self.emit(&[0x48, 0xF7, 0xF9]); // idiv rcx
        } else {
            self.emit(&[0x31, 0xD2]); // xor edx, edx
            self.emit(&[0x48, 0xF7, 0xF1]); // div rcx
        }
        if rem {
            self.emit(&[0x48, 0x89, 0xD0]); // mov rax, rdx
        }
        self.result(t)
    }

    pub fn udiv(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, false, false)
    }
    pub fn sdiv(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, true, false)
    }
    pub fn urem(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, false, true)
    }
    pub fn srem(&mut self, a: Value, b: Value) -> Value {
        self.divop(a, b, true, true)
    }

    /// シフト (量は rcx の下位 8 bit)
    fn shift(&mut self, a: Value, b: Value, op: u8, arith: bool) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        if arith {
            self.sext_rax(t);
        }
        self.load_rcx(b);
        self.emit(&[0x48, 0xD3, op]);
        self.result(t)
    }

    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        self.shift(a, b, 0xE0, false)
    }
    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        self.shift(a, b, 0xE8, false)
    }
    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        self.shift(a, b, 0xF8, true)
    }

    pub fn ineg(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0xF7, 0xD8]);
        self.result(t)
    }

    pub fn bnot(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0xF7, 0xD0]);
        self.result(t)
    }

    /// 比較 (結果は I8 の 0 / 1)
    pub fn icmp(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.load_rcx(b);
        if cc.signed() {
            self.sext_rax(t);
            self.sext_rcx(t);
        }
        self.emit(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
        self.emit(&[0x0F, cc.setcc(), 0xC0]); // setcc al
        self.emit(&[0x48, 0x0F, 0xB6, 0xC0]); // movzx rax, al
        self.result(Type::I8)
    }

    pub fn icmp_imm(&mut self, cc: IntCC, a: Value, x: i64) -> Value {
        let t = self.value_type(a);
        let k = self.iconst(t, x);
        self.icmp(cc, a, k)
    }

    /// c ? a : b
    pub fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        self.load_rcx(a);
        self.load_rax(b);
        let off = self.values[c.0 as usize].0;
        // cmp byte [rbp+off], 0; cmovne rax, rcx
        self.emit_disp(&[0x80, 0xBD], off);
        self.emit(&[0x00]);
        self.emit(&[0x48, 0x0F, 0x45, 0xC1]);
        self.result(t)
    }

    pub fn uextend(&mut self, t: Type, a: Value) -> Value {
        self.load_rax(a);
        self.result(t)
    }

    pub fn sextend(&mut self, t: Type, a: Value) -> Value {
        let st = self.value_type(a);
        self.load_rax(a);
        self.sext_rax(st);
        self.result(t)
    }

    pub fn ireduce(&mut self, t: Type, a: Value) -> Value {
        self.load_rax(a);
        self.result(t)
    }

    /// [addr + off] を読む
    pub fn load(&mut self, t: Type, addr: Value, off: i32) -> Value {
        self.load_rax(addr);
        match t {
            Type::I64 => self.emit_disp(&[0x48, 0x8B, 0x80], off),
            Type::I32 => self.emit_disp(&[0x8B, 0x80], off),
            Type::I16 => self.emit_disp(&[0x48, 0x0F, 0xB7, 0x80], off),
            Type::I8 => self.emit_disp(&[0x48, 0x0F, 0xB6, 0x80], off),
        }
        self.result(t)
    }

    /// [addr + off] に v を書く
    pub fn store(&mut self, v: Value, addr: Value, off: i32) {
        let t = self.value_type(v);
        self.load_rax(addr);
        self.load_rcx(v);
        match t {
            Type::I64 => self.emit_disp(&[0x48, 0x89, 0x88], off),
            Type::I32 => self.emit_disp(&[0x89, 0x88], off),
            Type::I16 => self.emit_disp(&[0x66, 0x89, 0x88], off),
            Type::I8 => self.emit_disp(&[0x88, 0x88], off),
        }
    }

    /// スタックスロットのアドレス
    pub fn stack_addr(&mut self, s: StackSlot, off: i32) -> Value {
        let base = self.slots[s.0 as usize];
        self.emit_disp(&[0x48, 0x8D, 0x85], base + off);
        self.result(Type::I64)
    }

    fn rel32_to(&mut self, b: Block) {
        let at = self.code.len();
        self.emit(&[0, 0, 0, 0]);
        self.fixups.push((at, b));
    }

    /// ブロック引数を渡す (飛び先の引数のスロットへ書く)
    fn pass_args(&mut self, b: Block, args: &[Value]) {
        let ps = self.blocks[b.0 as usize].params.clone();
        // (引数どうしが入れ替わる場合に備え、全部読んでから書く: スタックに積む)
        for &a in args {
            self.load_rax(a);
            self.emit(&[0x50]); // push rax
        }
        for &p in ps.iter().rev().take(args.len()) {
            self.emit(&[0x58]); // pop rax
            self.store_rax(p);
        }
    }

    pub fn jump(&mut self, b: Block, args: &[Value]) {
        self.pass_args(b, args);
        self.emit(&[0xE9]);
        self.rel32_to(b);
    }

    /// c が 0 でなければ then_b へ、0 なら else_b へ
    pub fn brif(&mut self, c: Value, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        let off = self.values[c.0 as usize].0;
        self.load_rax_at(off);
        self.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
        // jz → else 側の引数渡しへ
        self.emit(&[0x0F, 0x84]);
        let jz_at = self.code.len();
        self.emit(&[0, 0, 0, 0]);
        self.jump(then_b, then_args);
        let here = self.code.len() as i32;
        let rel = here - (jz_at as i32 + 4);
        self.code[jz_at..jz_at + 4].copy_from_slice(&rel.to_le_bytes());
        self.jump(else_b, else_args);
    }

    pub fn return_(&mut self, vals: &[Value]) {
        if let Some(&v) = vals.first() {
            self.load_rax(v);
        }
        if let Some(&v) = vals.get(1) {
            // 2 つ目は rdx
            let off = self.values[v.0 as usize].0;
            self.emit_disp(&[0x48, 0x8B, 0x95], off);
        }
        // leave; ret
        self.emit(&[0xC9, 0xC3]);
    }

    pub fn trap(&mut self) {
        self.emit(&[0x0F, 0x0B]); // ud2
    }

    /// 関数を呼ぶ (引数は 6 個まではレジスタ、それ以降はスタック)。戻り値は 0〜2 個 (rax, rdx)
    pub fn call(&mut self, f: FuncRef, args: &[Value], rets: &[Type]) -> Vec<Value> {
        const ARG_LOAD: [[u8; 3]; 6] = [[0x48, 0x8B, 0xBD], [0x48, 0x8B, 0xB5], [0x48, 0x8B, 0x95], [0x48, 0x8B, 0x8D], [0x4C, 0x8B, 0x85], [0x4C, 0x8B, 0x8D]];
        let stack_args: Vec<Value> = args.iter().skip(6).copied().collect();
        // (スタック渡しの引数: rsp を 16 byte に揃えてから逆順に push)
        let pad = stack_args.len() % 2 == 1;
        if pad {
            self.emit(&[0x48, 0x83, 0xEC, 0x08]); // sub rsp, 8
        }
        for &a in stack_args.iter().rev() {
            self.load_rax(a);
            self.emit(&[0x50]);
        }
        for (i, &a) in args.iter().take(6).enumerate() {
            let off = self.values[a.0 as usize].0;
            self.emit_disp(&ARG_LOAD[i], off);
        }
        self.emit(&[0x31, 0xC0]); // xor eax, eax (可変長引数の関数のための al = 0)
        self.emit(&[0xE8]);
        let at = self.code.len() as u32;
        self.emit(&[0, 0, 0, 0]);
        self.relocs.push(Reloc { offset: at, name: self.funcs[f.0 as usize].clone(), kind: 4, addend: -4 });
        let n = stack_args.len() as i32 + pad as i32;
        if n > 0 {
            self.emit_disp(&[0x48, 0x81, 0xC4], 8 * n); // add rsp, 8n
        }
        let mut out = vec![];
        for (i, &t) in rets.iter().enumerate() {
            if i == 1 {
                self.emit(&[0x48, 0x89, 0xD0]); // mov rax, rdx
            }
            out.push(self.result(t));
        }
        out
    }

    /// 組み立てを終えて (機械語, 再配置)
    pub fn finish(mut self) -> (Vec<u8>, Vec<Reloc>) {
        // フレームの大きさ (16 byte 境界)
        let frame = (self.frame + 15) / 16 * 16;
        self.code[7..11].copy_from_slice(&frame.to_le_bytes());
        for &(at, b) in &self.fixups {
            let target = self.blocks[b.0 as usize].offset.expect("jump to a block that was never switched to") as i32;
            let rel = target - (at as i32 + 4);
            self.code[at..at + 4].copy_from_slice(&rel.to_le_bytes());
        }
        let _ = (&self.params, &self.rets);
        (self.code, self.relocs)
    }
}

// --- ELF64 の再配置可能オブジェクト ------------------------------------------

/// 関数 (名前, 機械語, 再配置, 外から見えるか) を .text に並べた ELF の .o
pub fn write_object(funcs: &[(String, Vec<u8>, Vec<Reloc>, bool)]) -> Vec<u8> {
    // .text
    let mut text: Vec<u8> = vec![];
    let mut offs = vec![];
    for (_, code, _, _) in funcs {
        while text.len() % 16 != 0 {
            text.push(0xCC);
        }
        offs.push(text.len());
        text.extend_from_slice(code);
    }
    // 文字列表と記号表: 0 = null、1 = .text のセクション記号、ローカル関数、グローバル関数、未定義
    let mut strtab: Vec<u8> = vec![0];
    let add_str = |s: &str, t: &mut Vec<u8>| -> u32 {
        let o = t.len() as u32;
        t.extend_from_slice(s.as_bytes());
        t.push(0);
        o
    };
    struct Sym {
        name: u32,
        info: u8,
        shndx: u16,
        value: u64,
        size: u64,
    }
    let mut syms: Vec<Sym> = vec![Sym { name: 0, info: 0, shndx: 0, value: 0, size: 0 }, Sym { name: 0, info: 3, shndx: 1, value: 0, size: 0 }];
    let mut index: Vec<(String, u32)> = vec![];
    for pass in 0..2 {
        for (i, (name, code, _, global)) in funcs.iter().enumerate() {
            if *global != (pass == 1) {
                continue;
            }
            let n = add_str(name, &mut strtab);
            let bind = if *global { 1u8 } else { 0 };
            syms.push(Sym { name: n, info: (bind << 4) | 2, shndx: 1, value: offs[i] as u64, size: code.len() as u64 });
            index.push((name.clone(), syms.len() as u32 - 1));
        }
    }
    let first_global = syms.iter().position(|s| s.info >> 4 == 1).unwrap_or(syms.len()) as u32;
    // 未定義の記号 (呼ぶ外部の関数)
    for (_, _, rs, _) in funcs {
        for r in rs {
            if !index.iter().any(|x| x.0 == r.name) {
                let n = add_str(&r.name, &mut strtab);
                syms.push(Sym { name: n, info: (1 << 4) | 0, shndx: 0, value: 0, size: 0 });
                index.push((r.name.clone(), syms.len() as u32 - 1));
            }
        }
    }
    let first_global = first_global.min(syms.len() as u32);
    let mut symtab: Vec<u8> = vec![];
    for s in &syms {
        symtab.extend_from_slice(&s.name.to_le_bytes());
        symtab.push(s.info);
        symtab.push(0);
        symtab.extend_from_slice(&s.shndx.to_le_bytes());
        symtab.extend_from_slice(&s.value.to_le_bytes());
        symtab.extend_from_slice(&s.size.to_le_bytes());
    }
    // .rela.text
    let mut rela: Vec<u8> = vec![];
    for (i, (_, _, rs, _)) in funcs.iter().enumerate() {
        for r in rs {
            let si = index.iter().find(|x| x.0 == r.name).unwrap().1 as u64;
            rela.extend_from_slice(&((offs[i] as u64) + r.offset as u64).to_le_bytes());
            rela.extend_from_slice(&((si << 32) | r.kind as u64).to_le_bytes());
            rela.extend_from_slice(&r.addend.to_le_bytes());
        }
    }
    // (名前の位置: .text 1, .symtab 7, .strtab 15, .rela.text 23, .note.GNU-stack 34, .shstrtab 50)
    let shstr = b"\0.text\0.symtab\0.strtab\0.rela.text\0.note.GNU-stack\0.shstrtab\0".to_vec();
    // 配置: ヘッダ (64) → .text → .symtab → .strtab → .rela.text → .shstrtab → セクションヘッダ
    let mut out = vec![0u8; 64];
    let align = |o: &mut Vec<u8>, a: usize| {
        while o.len() % a != 0 {
            o.push(0);
        }
    };
    align(&mut out, 16);
    let text_off = out.len();
    out.extend_from_slice(&text);
    align(&mut out, 8);
    let sym_off = out.len();
    out.extend_from_slice(&symtab);
    let str_off = out.len();
    out.extend_from_slice(&strtab);
    align(&mut out, 8);
    let rela_off = out.len();
    out.extend_from_slice(&rela);
    let shstr_off = out.len();
    out.extend_from_slice(&shstr);
    align(&mut out, 8);
    let sh_off = out.len();
    // セクションヘッダ: null, .text(1), .symtab(2), .strtab(3), .rela.text(4), .shstrtab(5), .note.GNU-stack(6)
    let sh = |name: u32, ty: u32, flags: u64, off: usize, size: usize, link: u32, info: u32, al: u64, ent: u64, o: &mut Vec<u8>| {
        o.extend_from_slice(&name.to_le_bytes());
        o.extend_from_slice(&ty.to_le_bytes());
        o.extend_from_slice(&flags.to_le_bytes());
        o.extend_from_slice(&0u64.to_le_bytes());
        o.extend_from_slice(&(off as u64).to_le_bytes());
        o.extend_from_slice(&(size as u64).to_le_bytes());
        o.extend_from_slice(&link.to_le_bytes());
        o.extend_from_slice(&info.to_le_bytes());
        o.extend_from_slice(&al.to_le_bytes());
        o.extend_from_slice(&ent.to_le_bytes());
    };
    sh(0, 0, 0, 0, 0, 0, 0, 0, 0, &mut out);
    sh(1, 1, 6, text_off, text.len(), 0, 0, 16, 0, &mut out);
    sh(7, 2, 0, sym_off, symtab.len(), 3, first_global, 8, 24, &mut out);
    sh(15, 3, 0, str_off, strtab.len(), 0, 0, 1, 0, &mut out);
    sh(23, 4, 0x40, rela_off, rela.len(), 2, 1, 8, 24, &mut out);
    sh(50, 3, 0, shstr_off, 0, 0, 0, 1, 0, &mut out);
    sh(34, 1, 0, shstr_off, 0, 0, 0, 1, 0, &mut out);
    // (.shstrtab の大きさを後から入れる)
    let shstrtab_hdr = sh_off + 5 * 64;
    out[shstrtab_hdr + 32..shstrtab_hdr + 40].copy_from_slice(&(shstr.len() as u64).to_le_bytes());
    // ELF ヘッダ
    let mut h: Vec<u8> = vec![0x7F, b'E', b'L', b'F', 2, 1, 1, 0];
    h.extend_from_slice(&[0; 8]);
    h.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
    h.extend_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    h.extend_from_slice(&1u32.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes()); // entry
    h.extend_from_slice(&0u64.to_le_bytes()); // phoff
    h.extend_from_slice(&(sh_off as u64).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&64u16.to_le_bytes());
    h.extend_from_slice(&0u16.to_le_bytes());
    h.extend_from_slice(&0u16.to_le_bytes());
    h.extend_from_slice(&64u16.to_le_bytes());
    h.extend_from_slice(&7u16.to_le_bytes()); // shnum
    h.extend_from_slice(&5u16.to_le_bytes()); // shstrndx
    out[..64].copy_from_slice(&h);
    out
}
