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
    I128,
    F32,
    F64,
}

impl Type {
    pub fn bytes(self) -> u32 {
        match self {
            Type::I8 => 1,
            Type::I16 => 2,
            Type::I32 => 4,
            Type::I64 | Type::F64 => 8,
            Type::I128 => 16,
            Type::F32 => 4,
        }
    }
    pub fn is_float(self) -> bool {
        matches!(self, Type::F32 | Type::F64)
    }
    pub fn bits(self) -> u32 {
        self.bytes() * 8
    }
}

/// 浮動小数点の比較
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatCC {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    /// どちらかが NaN
    Unordered,
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
    /// trap の場所 (ud2 のオフセット) とコード (Cranelift の TrapCode::user に当たる)
    traps: Vec<(u32, u8)>,
    entry_jump: usize,
    /// 今のブロックが終端命令 (jump / brif / return / trap) で終わったか
    terminated: bool,
}

struct BlockData {
    params: Vec<Value>,
    /// コードの位置 (切り替えたとき)
    offset: Option<usize>,
    /// 終端命令の前に別のブロックへ切り替えたとき、続きへ飛ぶ jmp の rel32 の位置 (戻ってきたら今の位置へ向ける)
    cont: Option<usize>,
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
            traps: vec![],
            entry_jump: 0,
            terminated: false,
        };
        // プロローグ: push rbp; mov rbp, rsp; sub rsp, <frame> (frame は最後に書き込む)
        b.code.extend_from_slice(&[0x55, 0x48, 0x89, 0xE5, 0x48, 0x81, 0xEC, 0, 0, 0, 0]);
        // 引数をスロットへ
        const ARG_STORE: [[u8; 3]; 6] = [[0x48, 0x89, 0xBD], [0x48, 0x89, 0xB5], [0x48, 0x89, 0x95], [0x48, 0x89, 0x8D], [0x4C, 0x89, 0x85], [0x4C, 0x89, 0x8D]];
        let (mut ni, mut nf) = (0usize, 0u8);
        let mut ns = 0i32;
        for (_, &t) in params.iter().enumerate() {
            let v = b.new_value(t);
            let off = b.values[v.0 as usize].0;
            // (浮動小数点の引数は xmm0〜7)
            if t.is_float() && nf < 8 {
                let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
                b.code.extend_from_slice(&[p, 0x0F, 0x11, 0x85 | (nf << 3)]);
                b.code.extend_from_slice(&off.to_le_bytes());
                nf += 1;
                b.entry_params.push(v);
                continue;
            }
            // (i128 はレジスタ 2 つ (下位, 上位)。2 つ残っていなければ全体をスタックで)
            let parts = if t == Type::I128 { 2 } else { 1 };
            if ni + parts <= 6 {
                for k in 0..parts {
                    b.code.extend_from_slice(&ARG_STORE[ni + k]);
                    b.code.extend_from_slice(&(off + 8 * k as i32).to_le_bytes());
                }
                ni += parts;
            } else {
                ni = 6;
                for k in 0..parts {
                    // スタック渡し: [rbp + 16 + 8*n]
                    let src = 16 + 8 * ns;
                    ns += 1;
                    b.code.extend_from_slice(&[0x48, 0x8B, 0x85]);
                    b.code.extend_from_slice(&src.to_le_bytes());
                    b.store_rax_at(off + 8 * k as i32);
                }
            }
            b.entry_params.push(v);
        }
        // (入口で 0 にしたいスロットがあれば finish_zeroing がここから飛ぶ: 普段は次の命令へ飛ぶだけ)
        b.entry_jump = b.code.len();
        b.code.extend_from_slice(&[0xE9, 0, 0, 0, 0]);
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
        let off = if t == Type::I128 { self.alloc(16, 16) } else { self.alloc(8, 8) };
        self.values.push((off, t));
        Value(self.values.len() as u32 - 1)
    }

    pub fn value_type(&self, v: Value) -> Type {
        self.values[v.0 as usize].1
    }

    pub fn create_block(&mut self) -> Block {
        self.blocks.push(BlockData { params: vec![], offset: None, cont: None });
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

    /// ブロックに切り替える。途中のブロック (終端命令がまだない) から離れるときは続きへの jmp を置き、
    /// 後でそのブロックに戻ったら (Cranelift と同じく続きを書ける) その jmp を今の位置へ向ける
    pub fn switch_to_block(&mut self, b: Block) {
        if let Some(c) = self.cur {
            if !self.terminated && c != b {
                self.emit(&[0xE9]);
                let at = self.code.len();
                self.emit(&[0, 0, 0, 0]);
                self.blocks[c.0 as usize].cont = Some(at);
            }
        }
        let here = self.code.len();
        let bd = &mut self.blocks[b.0 as usize];
        if let Some(at) = bd.cont.take() {
            let rel = here as i32 - (at as i32 + 4);
            self.code[at..at + 4].copy_from_slice(&rel.to_le_bytes());
        } else if bd.offset.is_none() {
            bd.offset = Some(here);
        }
        self.cur = Some(b);
        self.terminated = false;
    }

    pub fn current_block(&self) -> Option<Block> {
        self.cur
    }

    pub fn declare_var(&mut self, t: Type) -> Variable {
        let off = if t == Type::I128 { self.alloc(16, 16) } else { self.alloc(8, 8) };
        self.vars.push((off, t));
        Variable(self.vars.len() as u32 - 1)
    }

    pub fn def_var(&mut self, var: Variable, v: Value) {
        let (off, t) = self.vars[var.0 as usize];
        self.load_rax(v);
        self.store_rax_at(off);
        if t == Type::I128 {
            let o = self.values[v.0 as usize].0;
            self.load_rax_at(o + 8);
            self.store_rax_at(off + 8);
        }
    }

    pub fn use_var(&mut self, var: Variable) -> Value {
        let (off, t) = self.vars[var.0 as usize];
        let v = self.new_value(t);
        self.load_rax_at(off);
        self.store_rax(v);
        if t == Type::I128 {
            let o = self.values[v.0 as usize].0;
            self.load_rax_at(off + 8);
            self.store_rax_at(o + 8);
        }
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
            _ => {}
        }
    }

    /// rax を型の幅から符号拡張する
    fn sext_rax(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xBE, 0xC0]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xBF, 0xC0]),
            Type::I32 => self.emit(&[0x48, 0x63, 0xC0]),
            _ => {}
        }
    }

    fn sext_rcx(&mut self, t: Type) {
        match t {
            Type::I8 => self.emit(&[0x48, 0x0F, 0xBE, 0xC9]),
            Type::I16 => self.emit(&[0x48, 0x0F, 0xBF, 0xC9]),
            Type::I32 => self.emit(&[0x48, 0x63, 0xC9]),
            _ => {}
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
            Type::I64 | Type::F64 => self.emit_disp(&[0x48, 0x8B, 0x80], off),
            Type::I32 | Type::F32 => self.emit_disp(&[0x8B, 0x80], off),
            Type::I16 => self.emit_disp(&[0x48, 0x0F, 0xB7, 0x80], off),
            Type::I8 => self.emit_disp(&[0x48, 0x0F, 0xB6, 0x80], off),
            Type::I128 => {
                // 下位を rcx、上位を rax に読んで書く
                self.emit_disp(&[0x48, 0x8B, 0x88], off);
                self.emit_disp(&[0x48, 0x8B, 0x80], off + 8);
                let v = self.new_value(t);
                let o = self.values[v.0 as usize].0;
                self.emit_disp(&[0x48, 0x89, 0x8D], o);
                self.store_rax_at(o + 8);
                return v;
            }
        }
        self.result(t)
    }

    /// [addr + off] に v を書く
    pub fn store(&mut self, v: Value, addr: Value, off: i32) {
        let t = self.value_type(v);
        self.load_rax(addr);
        self.load_rcx(v);
        match t {
            Type::I64 | Type::F64 => self.emit_disp(&[0x48, 0x89, 0x88], off),
            Type::I32 | Type::F32 => self.emit_disp(&[0x89, 0x88], off),
            Type::I16 => self.emit_disp(&[0x66, 0x89, 0x88], off),
            Type::I8 => self.emit_disp(&[0x88, 0x88], off),
            Type::I128 => {
                self.emit_disp(&[0x48, 0x89, 0x88], off);
                let o = self.values[v.0 as usize].0;
                self.emit_disp(&[0x48, 0x8B, 0x8D], o + 8);
                self.emit_disp(&[0x48, 0x89, 0x88], off + 8);
            }
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
            if self.value_type(a) == Type::I128 {
                let o = self.values[a.0 as usize].0;
                self.load_rax_at(o + 8);
                self.emit(&[0x50]);
            }
        }
        for &p in ps.iter().rev().take(args.len()) {
            if self.value_type(p) == Type::I128 {
                let o = self.values[p.0 as usize].0;
                self.emit(&[0x58]);
                self.store_rax_at(o + 8);
            }
            self.emit(&[0x58]); // pop rax
            self.store_rax(p);
        }
    }

    pub fn jump(&mut self, b: Block, args: &[Value]) {
        self.pass_args(b, args);
        self.emit(&[0xE9]);
        self.rel32_to(b);
        self.terminated = true;
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
            if self.value_type(v).is_float() {
                self.xmm_load(0, v);
            } else {
                self.load_rax(v);
            }
            if self.value_type(v) == Type::I128 {
                // (上位は rdx)
                let off = self.values[v.0 as usize].0;
                self.emit_disp(&[0x48, 0x8B, 0x95], off + 8);
            }
        }
        if let Some(&v) = vals.get(1) {
            // 2 つ目は rdx
            let off = self.values[v.0 as usize].0;
            self.emit_disp(&[0x48, 0x8B, 0x95], off);
        }
        // leave; ret
        self.emit(&[0xC9, 0xC3]);
        self.terminated = true;
    }

    pub fn trap(&mut self) {
        self.emit(&[0x0F, 0x0B]); // ud2
        self.terminated = true;
    }

    /// 関数を呼ぶ (引数は 6 個まではレジスタ、それ以降はスタック)。戻り値は 0〜2 個 (rax, rdx)
    pub fn call(&mut self, f: FuncRef, args: &[Value], rets: &[Type]) -> Vec<Value> {
        const ARG_LOAD: [[u8; 3]; 6] = [[0x48, 0x8B, 0xBD], [0x48, 0x8B, 0xB5], [0x48, 0x8B, 0x95], [0x48, 0x8B, 0x8D], [0x4C, 0x8B, 0x85], [0x4C, 0x8B, 0x8D]];
        let fargs: Vec<Value> = args.iter().copied().filter(|&a| self.value_type(a).is_float()).collect();
        let args: Vec<Value> = args.iter().copied().filter(|&a| !self.value_type(a).is_float()).collect();
        // 8 byte ずつの部分 (スロットのオフセット) をレジスタとスタックに分ける (i128 は 2 つ。2 つ残っていなければ全体をスタック)
        let mut regs: Vec<i32> = vec![];
        let mut stack_parts: Vec<i32> = vec![];
        for &a in &args {
            let off = self.values[a.0 as usize].0;
            let parts: Vec<i32> = if self.value_type(a) == Type::I128 { vec![off, off + 8] } else { vec![off] };
            if regs.len() + parts.len() <= 6 && stack_parts.is_empty() {
                regs.extend(parts);
            } else {
                stack_parts.extend(parts);
            }
        }
        // (スタック渡しの引数: rsp を 16 byte に揃えてから逆順に push)
        let pad = stack_parts.len() % 2 == 1;
        if pad {
            self.emit(&[0x48, 0x83, 0xEC, 0x08]); // sub rsp, 8
        }
        for &o in stack_parts.iter().rev() {
            self.load_rax_at(o);
            self.emit(&[0x50]);
        }
        for (i, &off) in regs.iter().enumerate() {
            self.emit_disp(&ARG_LOAD[i], off);
        }
        for (k, &a) in fargs.iter().enumerate().take(8) {
            self.xmm_load(k as u8, a);
        }
        // (可変長引数の関数のための al = xmm で渡した数)
        self.emit(&[0xB8]);
        self.emit(&(fargs.len().min(8) as u32).to_le_bytes());
        self.emit(&[0xE8]);
        let at = self.code.len() as u32;
        self.emit(&[0, 0, 0, 0]);
        self.relocs.push(Reloc { offset: at, name: self.funcs[f.0 as usize].clone(), kind: 4, addend: -4 });
        let n = stack_parts.len() as i32 + pad as i32;
        if n > 0 {
            self.emit_disp(&[0x48, 0x81, 0xC4], 8 * n); // add rsp, 8n
        }
        let mut out = vec![];
        for (i, &t) in rets.iter().enumerate() {
            if t.is_float() {
                out.push(self.xmm0_result(t));
                continue;
            }
            if t == Type::I128 {
                // rax (下位) と rdx (上位)
                let v = self.new_value(t);
                let o = self.values[v.0 as usize].0;
                self.store_rax_at(o);
                self.emit_disp(&[0x48, 0x89, 0x95], o + 8); // mov [rbp+o+8], rdx
                out.push(v);
                continue;
            }
            if i == 1 {
                self.emit(&[0x48, 0x89, 0xD0]); // mov rax, rdx
            }
            out.push(self.result(t));
        }
        out
    }

    /// 組み立てを終えて (機械語, 再配置)。zero のスロットの先頭 1 byte を入口で 0 にする
    pub fn finish_zeroing(mut self, zero: &[StackSlot]) -> (Vec<u8>, Vec<Reloc>) {
        if !zero.is_empty() {
            let stub = self.code.len();
            for s in zero {
                let off = self.slots[s.0 as usize];
                self.emit_disp(&[0xC6, 0x85], off); // mov byte [rbp+off], 0
                self.emit(&[0]);
            }
            let back = self.entry_jump + 5;
            let at = self.code.len();
            self.emit(&[0xE9]);
            let rel = back as i32 - (at as i32 + 5);
            self.emit(&rel.to_le_bytes());
            let rel = stub as i32 - (self.entry_jump as i32 + 5);
            let j = self.entry_jump;
            self.code[j + 1..j + 5].copy_from_slice(&rel.to_le_bytes());
        }
        self.finish()
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


// --- 浮動小数点・i128・オーバーフロー・ビット演算・間接呼び出し・アドレス ---------------
impl FunctionBuilder {
    fn off(&self, v: Value) -> i32 {
        self.values[v.0 as usize].0
    }

    /// xmm0 (reg 0) / xmm1 (reg 1) に読む
    fn xmm_load(&mut self, reg: u8, v: Value) {
        let p = if self.value_type(v) == Type::F32 { 0xF3 } else { 0xF2 };
        let o = self.off(v);
        self.emit_disp(&[p, 0x0F, 0x10, 0x85 | (reg << 3)], o);
    }

    fn xmm0_result(&mut self, t: Type) -> Value {
        let v = self.new_value(t);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        let o = self.off(v);
        // (スロットの上位を 0 にしてから書く: f32 も 8 byte のスロットで比べられるように)
        self.emit(&[0x31, 0xC0]);
        self.store_rax_at(o);
        self.emit_disp(&[p, 0x0F, 0x11, 0x85], o);
        v
    }

    pub fn f64const(&mut self, x: f64) -> Value {
        self.emit(&[0x48, 0xB8]);
        self.emit(&x.to_bits().to_le_bytes());
        let v = self.new_value(Type::F64);
        self.store_rax(v);
        v
    }

    pub fn f32const(&mut self, x: f32) -> Value {
        self.emit(&[0x48, 0xB8]);
        self.emit(&(x.to_bits() as u64).to_le_bytes());
        let v = self.new_value(Type::F32);
        self.store_rax(v);
        v
    }

    fn fop(&mut self, a: Value, b: Value, opc: u8) -> Value {
        let t = self.value_type(a);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        self.xmm_load(0, a);
        let o = self.off(b);
        self.emit_disp(&[p, 0x0F, opc, 0x85], o);
        self.xmm0_result(t)
    }

    pub fn fadd(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x58)
    }
    pub fn fsub(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x5C)
    }
    pub fn fmul(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x59)
    }
    pub fn fdiv(&mut self, a: Value, b: Value) -> Value {
        self.fop(a, b, 0x5E)
    }

    pub fn sqrt(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        self.xmm_load(0, a);
        self.emit(&[p, 0x0F, 0x51, 0xC0]);
        self.xmm0_result(t)
    }

    /// 切り捨て・切り上げ・0 方向への丸め (roundsd の即値 9 / 10 / 11)
    fn round(&mut self, a: Value, mode: u8) -> Value {
        let t = self.value_type(a);
        self.xmm_load(0, a);
        let op = if t == Type::F32 { 0x0A } else { 0x0B };
        self.emit(&[0x66, 0x0F, 0x3A, op, 0xC0, mode]);
        self.xmm0_result(t)
    }
    pub fn floor(&mut self, a: Value) -> Value {
        self.round(a, 9)
    }
    pub fn ceil(&mut self, a: Value) -> Value {
        self.round(a, 10)
    }
    pub fn trunc(&mut self, a: Value) -> Value {
        self.round(a, 11)
    }

    fn sign_mask(t: Type) -> i64 {
        if t == Type::F32 { 1 << 31 } else { i64::MIN }
    }

    /// 浮動小数点のビット列をそのまま整数として扱う (I32 / I64)
    fn bits_of(&mut self, a: Value) -> Value {
        let t = if self.value_type(a) == Type::F32 { Type::I32 } else { Type::I64 };
        self.bitcast(t, a)
    }

    pub fn fneg(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        let x = self.bits_of(a);
        let m = self.iconst(self.value_type(x), Self::sign_mask(t));
        let r = self.bxor(x, m);
        self.bitcast(t, r)
    }

    pub fn fabs(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        let x = self.bits_of(a);
        let m = self.iconst(self.value_type(x), !Self::sign_mask(t));
        let r = self.band(x, m);
        self.bitcast(t, r)
    }

    pub fn fcopysign(&mut self, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        let (x, y) = (self.bits_of(a), self.bits_of(b));
        let it = self.value_type(x);
        let m = self.iconst(it, Self::sign_mask(t));
        let nm = self.iconst(it, !Self::sign_mask(t));
        let x2 = self.band(x, nm);
        let y2 = self.band(y, m);
        let r = self.bor(x2, y2);
        self.bitcast(t, r)
    }

    /// a * b + c (丸めは 2 回: 融合はしない)
    pub fn fma(&mut self, a: Value, b: Value, c: Value) -> Value {
        let m = self.fmul(a, b);
        self.fadd(m, c)
    }

    /// 型だけを変える (ビット列はそのまま)
    pub fn bitcast(&mut self, t: Type, a: Value) -> Value {
        self.load_rax(a);
        let v = self.new_value(t);
        self.store_rax(v);
        v
    }

    /// 比較 (NaN を含むと Equal 以外の順序の比較は偽、NotEqual は真)
    pub fn fcmp(&mut self, cc: FloatCC, a: Value, b: Value) -> Value {
        let t = self.value_type(a);
        let (x, y) = match cc {
            FloatCC::LessThan | FloatCC::LessThanOrEqual => (b, a),
            _ => (a, b),
        };
        self.xmm_load(0, x);
        let o = self.off(y);
        if t == Type::F32 {
            self.emit_disp(&[0x0F, 0x2E, 0x85], o);
        } else {
            self.emit_disp(&[0x66, 0x0F, 0x2E, 0x85], o);
        }
        match cc {
            FloatCC::GreaterThan | FloatCC::LessThan => self.emit(&[0x0F, 0x97, 0xC0]),
            FloatCC::GreaterThanOrEqual | FloatCC::LessThanOrEqual => self.emit(&[0x0F, 0x93, 0xC0]),
            // sete al; setnp cl; and al, cl
            FloatCC::Equal => self.emit(&[0x0F, 0x94, 0xC0, 0x0F, 0x9B, 0xC1, 0x20, 0xC8]),
            // setne al; setp cl; or al, cl
            FloatCC::NotEqual => self.emit(&[0x0F, 0x95, 0xC0, 0x0F, 0x9A, 0xC1, 0x08, 0xC8]),
            // setp al
            FloatCC::Unordered => self.emit(&[0x0F, 0x9A, 0xC0]),
        }
        self.emit(&[0x48, 0x0F, 0xB6, 0xC0]);
        self.result(Type::I8)
    }

    pub fn fpromote(&mut self, a: Value) -> Value {
        self.xmm_load(0, a);
        self.emit(&[0xF3, 0x0F, 0x5A, 0xC0]);
        self.xmm0_result(Type::F64)
    }

    pub fn fdemote(&mut self, a: Value) -> Value {
        self.xmm_load(0, a);
        self.emit(&[0xF2, 0x0F, 0x5A, 0xC0]);
        self.xmm0_result(Type::F32)
    }

    /// 符号つき整数 → 浮動小数点
    pub fn fcvt_from_sint(&mut self, t: Type, a: Value) -> Value {
        let st = self.value_type(a);
        self.load_rax(a);
        self.sext_rax(st);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        self.emit(&[p, 0x48, 0x0F, 0x2A, 0xC0]);
        self.xmm0_result(t)
    }

    /// 符号なし整数 → 浮動小数点 (64 bit で最上位が立っていれば半分にして 2 倍する)
    pub fn fcvt_from_uint(&mut self, t: Type, a: Value) -> Value {
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        let add = if t == Type::F32 { 0x58 } else { 0x58 };
        self.load_rax(a);
        // test rax, rax; js big
        self.emit(&[0x48, 0x85, 0xC0, 0x78, 0x07]);
        self.emit(&[p, 0x48, 0x0F, 0x2A, 0xC0]); // 5 byte
        self.emit(&[0xEB, 0x15]); // jmp done (2 byte)
        // big: mov rcx, rax; shr rcx, 1; and eax, 1; or rcx, rax; cvtsi2sd xmm0, rcx; addsd xmm0, xmm0
        self.emit(&[0x48, 0x89, 0xC1, 0x48, 0xD1, 0xE9, 0x83, 0xE0, 0x01, 0x48, 0x09, 0xC1]); // 12
        self.emit(&[p, 0x48, 0x0F, 0x2A, 0xC1]); // 5
        self.emit(&[p, 0x0F, add, 0xC0]); // 4
        self.xmm0_result(t)
    }

    /// 浮動小数点 → I64 (cvttsd2si: 範囲外と NaN は i64::MIN)
    fn cvt_raw(&mut self, a: Value) -> Value {
        let p = if self.value_type(a) == Type::F32 { 0xF3 } else { 0xF2 };
        self.xmm_load(0, a);
        self.emit(&[p, 0x48, 0x0F, 0x2C, 0xC0]);
        let v = self.new_value(Type::I64);
        self.store_rax(v);
        v
    }

    fn fconst_like(&mut self, a: Value, x: f64) -> Value {
        if self.value_type(a) == Type::F32 { self.f32const(x as f32) } else { self.f64const(x) }
    }

    /// 浮動小数点 → 符号つき整数 (Rust の `as`: 飽和し、NaN は 0)
    pub fn fcvt_to_sint_sat(&mut self, t: Type, a: Value) -> Value {
        let n = t.bits() as i32;
        let lo = -(2f64.powi(n - 1));
        let hi = 2f64.powi(n - 1);
        let raw = self.cvt_raw(a);
        let flo = self.fconst_like(a, lo);
        let fhi = self.fconst_like(a, hi);
        let too_low = self.fcmp(FloatCC::LessThan, a, flo);
        let too_high = self.fcmp(FloatCC::GreaterThanOrEqual, a, fhi);
        let nan = self.fcmp(FloatCC::NotEqual, a, a);
        let ilo = self.iconst(Type::I64, if n == 64 { i64::MIN } else { -(1i64 << (n - 1)) });
        let ihi = self.iconst(Type::I64, if n == 64 { i64::MAX } else { (1i64 << (n - 1)) - 1 });
        let z = self.iconst(Type::I64, 0);
        let r1 = self.select(too_high, ihi, raw);
        let r2 = self.select(too_low, ilo, r1);
        let r3 = self.select(nan, z, r2);
        self.ireduce(t, r3)
    }

    /// 浮動小数点 → 符号なし整数 (飽和し、NaN と負は 0)
    pub fn fcvt_to_uint_sat(&mut self, t: Type, a: Value) -> Value {
        let n = t.bits() as i32;
        let hi = 2f64.powi(n);
        let fz = self.fconst_like(a, 0.0);
        let fhi = self.fconst_like(a, hi);
        let f63 = self.fconst_like(a, 2f64.powi(63));
        // (2^63 以上は 2^63 を引いて変換し、最上位を立てる)
        let raw_small = self.cvt_raw(a);
        let a2 = self.fsub(a, f63);
        let raw_big0 = self.cvt_raw(a2);
        let top = self.iconst(Type::I64, i64::MIN);
        let raw_big = self.bxor(raw_big0, top);
        let big = self.fcmp(FloatCC::GreaterThanOrEqual, a, f63);
        let raw = self.select(big, raw_big, raw_small);
        let too_low = self.fcmp(FloatCC::LessThan, a, fz);
        let too_high = self.fcmp(FloatCC::GreaterThanOrEqual, a, fhi);
        let nan = self.fcmp(FloatCC::NotEqual, a, a);
        let ihi = self.iconst(Type::I64, if n == 64 { -1 } else { ((1u64 << n) - 1) as i64 });
        let z = self.iconst(Type::I64, 0);
        let r1 = self.select(too_high, ihi, raw);
        let r2 = self.select(too_low, z, r1);
        let r3 = self.select(nan, z, r2);
        self.ireduce(t, r3)
    }

    // --- i128 ---------------------------------------------------------

    /// (下位, 上位) から i128
    pub fn iconcat(&mut self, lo: Value, hi: Value) -> Value {
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.load_rax(lo);
        self.store_rax_at(o);
        self.load_rax(hi);
        self.store_rax_at(o + 8);
        v
    }

    /// i128 の (下位, 上位)
    pub fn isplit(&mut self, v: Value) -> (Value, Value) {
        let o = self.off(v);
        self.load_rax_at(o);
        let lo = self.new_value(Type::I64);
        self.store_rax(lo);
        self.load_rax_at(o + 8);
        let hi = self.new_value(Type::I64);
        self.store_rax(hi);
        (lo, hi)
    }

    /// 2 語の演算: rax = a.lo OP1 b.lo、rcx = a.hi OP2 b.hi (OP は [rbp + d] を取る形の 2 byte のオペコード)
    fn wide2(&mut self, a: Value, b: Value, op_lo: [u8; 2], op_hi: [u8; 2]) -> Value {
        let (oa, ob) = (self.off(a), self.off(b));
        self.load_rax_at(oa);
        self.emit_disp(&[op_lo[0], op_lo[1], 0x85], ob);
        // (adc / sbb は上の演算のキャリーを使うので、mov は flags を変えない)
        self.emit_disp(&[0x48, 0x8B, 0x8D], oa + 8);
        self.emit_disp(&[op_hi[0], op_hi[1], 0x8D], ob + 8);
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.store_rax_at(o);
        self.emit_disp(&[0x48, 0x89, 0x8D], o + 8);
        v
    }

    pub fn iadd128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x03], [0x48, 0x13])
    }
    pub fn isub128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x2B], [0x48, 0x1B])
    }
    pub fn band128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x23], [0x48, 0x23])
    }
    pub fn bor128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x0B], [0x48, 0x0B])
    }
    pub fn bxor128(&mut self, a: Value, b: Value) -> Value {
        self.wide2(a, b, [0x48, 0x33], [0x48, 0x33])
    }

    /// libgcc の関数 (`__multi3` など) で i128 の 2 項演算
    fn wide_libcall(&mut self, name: &str, a: Value, b: Value) -> Value {
        let (al, ah) = self.isplit(a);
        let (bl, bh) = self.isplit(b);
        let f = self.import_function(name);
        let r = self.call(f, &[al, ah, bl, bh], &[Type::I64, Type::I64]);
        self.iconcat(r[0], r[1])
    }
    pub fn imul128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__multi3", a, b)
    }
    pub fn udiv128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__udivti3", a, b)
    }
    pub fn sdiv128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__divti3", a, b)
    }
    pub fn urem128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__umodti3", a, b)
    }
    pub fn srem128(&mut self, a: Value, b: Value) -> Value {
        self.wide_libcall("__modti3", a, b)
    }

    /// i128 の比較
    pub fn icmp128(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        let (oa, ob) = (self.off(a), self.off(b));
        match cc {
            IntCC::Equal | IntCC::NotEqual => {
                self.load_rax_at(oa);
                self.emit_disp(&[0x48, 0x33, 0x85], ob); // xor rax, [b]
                self.emit_disp(&[0x48, 0x8B, 0x8D], oa + 8);
                self.emit_disp(&[0x48, 0x33, 0x8D], ob + 8); // xor rcx, [b+8]
                self.emit(&[0x48, 0x09, 0xC8]); // or rax, rcx
                let set = if cc == IntCC::Equal { 0x94 } else { 0x95 };
                self.emit(&[0x0F, set, 0xC0, 0x48, 0x0F, 0xB6, 0xC0]);
                self.result(Type::I8)
            }
            _ => {
                // x < y を sub / sbb の flags で: (a < b)、b < a、!(a < b)、!(b < a)
                let (x, y, neg) = match cc {
                    IntCC::SignedLessThan | IntCC::UnsignedLessThan => (oa, ob, false),
                    IntCC::SignedGreaterThan | IntCC::UnsignedGreaterThan => (ob, oa, false),
                    IntCC::SignedGreaterThanOrEqual | IntCC::UnsignedGreaterThanOrEqual => (oa, ob, true),
                    _ => (ob, oa, true),
                };
                self.load_rax_at(x);
                self.emit_disp(&[0x48, 0x2B, 0x85], y); // sub rax, [y]
                self.load_rax_at(x + 8);
                self.emit_disp(&[0x48, 0x1B, 0x85], y + 8); // sbb rax, [y+8]
                let set = if cc.signed() { if neg { 0x9D } else { 0x9C } } else if neg { 0x93 } else { 0x92 };
                self.emit(&[0x0F, set, 0xC0, 0x48, 0x0F, 0xB6, 0xC0]);
                self.result(Type::I8)
            }
        }
    }

    /// i128 のシフト (量は下位 7 bit)
    fn shift128(&mut self, a: Value, b: Value, kind: u8) -> Value {
        let oa = self.off(a);
        self.load_rax_at(oa);
        self.emit_disp(&[0x48, 0x8B, 0x95], oa + 8); // mov rdx, [a+8]
        self.load_rcx(b);
        match kind {
            // shld rdx, rax, cl; shl rax, cl; test cl, 64; jz +5; mov rdx, rax; xor eax, eax
            0 => self.emit(&[0x48, 0x0F, 0xA5, 0xC2, 0x48, 0xD3, 0xE0, 0xF6, 0xC1, 0x40, 0x74, 0x05, 0x48, 0x89, 0xC2, 0x31, 0xC0]),
            // shrd rax, rdx, cl; shr rdx, cl; test cl, 64; jz +5; mov rax, rdx; xor edx, edx
            1 => self.emit(&[0x48, 0x0F, 0xAD, 0xD0, 0x48, 0xD3, 0xEA, 0xF6, 0xC1, 0x40, 0x74, 0x05, 0x48, 0x89, 0xD0, 0x31, 0xD2]),
            // shrd rax, rdx, cl; sar rdx, cl; test cl, 64; jz +7; mov rax, rdx; sar rdx, 63
            _ => self.emit(&[0x48, 0x0F, 0xAD, 0xD0, 0x48, 0xD3, 0xFA, 0xF6, 0xC1, 0x40, 0x74, 0x07, 0x48, 0x89, 0xD0, 0x48, 0xC1, 0xFA, 0x3F]),
        }
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.store_rax_at(o);
        self.emit_disp(&[0x48, 0x89, 0x95], o + 8); // mov [v+8], rdx
        v
    }
    pub fn ishl128(&mut self, a: Value, b: Value) -> Value {
        self.shift128(a, b, 0)
    }
    pub fn ushr128(&mut self, a: Value, b: Value) -> Value {
        self.shift128(a, b, 1)
    }
    pub fn sshr128(&mut self, a: Value, b: Value) -> Value {
        self.shift128(a, b, 2)
    }

    /// 整数 → i128 (0 拡張 / 符号拡張)
    pub fn extend128(&mut self, a: Value, signed: bool) -> Value {
        let st = self.value_type(a);
        self.load_rax(a);
        if signed {
            self.sext_rax(st);
        }
        let v = self.new_value(Type::I128);
        let o = self.off(v);
        self.store_rax_at(o);
        if signed {
            self.emit(&[0x48, 0xC1, 0xF8, 0x3F]); // sar rax, 63
        } else {
            self.emit(&[0x31, 0xC0]);
        }
        self.store_rax_at(o + 8);
        v
    }

    /// i128 の下位を型の幅に切る
    pub fn reduce128(&mut self, t: Type, a: Value) -> Value {
        let o = self.off(a);
        self.load_rax_at(o);
        self.result(t)
    }

    // --- オーバーフローつきの演算: (結果, あふれたか (I8)) -------------------------

    pub fn uadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.iadd(a, b);
        let of = self.icmp(IntCC::UnsignedLessThan, v, a);
        (v, of)
    }

    pub fn usub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.isub(a, b);
        let of = self.icmp(IntCC::UnsignedLessThan, a, b);
        (v, of)
    }

    pub fn sadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.iadd(a, b);
        let x = self.bxor(a, v);
        let y = self.bxor(b, v);
        let m = self.band(x, y);
        let of = self.icmp_imm(IntCC::SignedLessThan, m, 0);
        (v, of)
    }

    pub fn ssub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let v = self.isub(a, b);
        let x = self.bxor(a, b);
        let y = self.bxor(a, v);
        let m = self.band(x, y);
        let of = self.icmp_imm(IntCC::SignedLessThan, m, 0);
        (v, of)
    }

    pub fn umul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        if t == Type::I64 {
            self.load_rax(a);
            self.load_rcx(b);
            self.emit(&[0x48, 0xF7, 0xE1]); // mul rcx (CF / OF = 上位が 0 でない)
            self.emit(&[0x0F, 0x92, 0xC1]); // setc cl
            let v = self.new_value(Type::I64);
            self.store_rax(v);
            self.emit(&[0x48, 0x0F, 0xB6, 0xC1]); // movzx rax, cl
            let of = self.result(Type::I8);
            return (v, of);
        }
        // 狭い型: 64 bit で掛けて、上の桁が残るか
        let (xa, xb) = (self.uextend(Type::I64, a), self.uextend(Type::I64, b));
        let p = self.imul(xa, xb);
        let sh = self.iconst(Type::I64, t.bits() as i64);
        let hi = self.ushr(p, sh);
        let of = self.icmp_imm(IntCC::NotEqual, hi, 0);
        let v = self.ireduce(t, p);
        (v, of)
    }

    pub fn smul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        let t = self.value_type(a);
        if t == Type::I64 {
            self.load_rax(a);
            self.load_rcx(b);
            self.emit(&[0x48, 0x0F, 0xAF, 0xC1]); // imul rax, rcx
            self.emit(&[0x0F, 0x90, 0xC1]); // seto cl
            let v = self.new_value(Type::I64);
            self.store_rax(v);
            self.emit(&[0x48, 0x0F, 0xB6, 0xC1]);
            let of = self.result(Type::I8);
            return (v, of);
        }
        let (xa, xb) = (self.sextend(Type::I64, a), self.sextend(Type::I64, b));
        let p = self.imul(xa, xb);
        let v = self.ireduce(t, p);
        let back = self.sextend(Type::I64, v);
        let of = self.icmp(IntCC::NotEqual, back, p);
        (v, of)
    }

    /// 符号なしの 64 bit 掛け算の上位
    pub fn umulhi(&mut self, a: Value, b: Value) -> Value {
        self.load_rax(a);
        self.load_rcx(b);
        self.emit(&[0x48, 0xF7, 0xE1, 0x48, 0x89, 0xD0]); // mul rcx; mov rax, rdx
        self.result(Type::I64)
    }

    // --- ビット演算・最小最大 -----------------------------------------------

    /// 上位の 0 の数 (0 なら幅)
    pub fn clz(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0x0F, 0xBD, 0xC0]); // bsr rax, rax (0 なら ZF)
        // jnz +7; mov rax, -1 (7 byte)
        self.emit(&[0x75, 0x07, 0x48, 0xC7, 0xC0, 0xFF, 0xFF, 0xFF, 0xFF]);
        // rax = (bits - 1) - rax
        self.emit(&[0x48, 0xF7, 0xD8]); // neg rax
        self.emit(&[0x48, 0x83, 0xC0, (t.bits() - 1) as u8]); // add rax, bits - 1
        self.result(t)
    }

    /// 下位の 0 の数 (0 なら幅)
    pub fn ctz(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0x48, 0x0F, 0xBC, 0xC0]); // bsf rax, rax
        // jnz +7; mov rax, bits
        self.emit(&[0x75, 0x07, 0x48, 0xC7, 0xC0]);
        self.emit(&(t.bits()).to_le_bytes());
        self.result(t)
    }

    pub fn popcnt(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.emit(&[0xF3, 0x48, 0x0F, 0xB8, 0xC0]);
        self.result(t)
    }

    pub fn bswap(&mut self, a: Value) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        match t {
            Type::I16 => self.emit(&[0x66, 0xC1, 0xC0, 0x08]), // rol ax, 8
            Type::I32 => self.emit(&[0x0F, 0xC8]),
            _ => self.emit(&[0x48, 0x0F, 0xC8]),
        }
        self.result(t)
    }

    fn rot(&mut self, a: Value, b: Value, ext: u8) -> Value {
        let t = self.value_type(a);
        self.load_rax(a);
        self.load_rcx(b);
        match t {
            Type::I8 => self.emit(&[0xD2, 0xC0 | ext]),
            Type::I16 => self.emit(&[0x66, 0xD3, 0xC0 | ext]),
            Type::I32 => self.emit(&[0xD3, 0xC0 | ext]),
            _ => self.emit(&[0x48, 0xD3, 0xC0 | ext]),
        }
        self.result(t)
    }
    pub fn rotl(&mut self, a: Value, b: Value) -> Value {
        self.rot(a, b, 0x00)
    }
    pub fn rotr(&mut self, a: Value, b: Value) -> Value {
        self.rot(a, b, 0x08)
    }

    fn minmax(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        let c = self.icmp(cc, a, b);
        self.select(c, a, b)
    }
    pub fn smin(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::SignedLessThan, a, b)
    }
    pub fn umin(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::UnsignedLessThan, a, b)
    }
    pub fn smax(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::SignedGreaterThan, a, b)
    }
    pub fn umax(&mut self, a: Value, b: Value) -> Value {
        self.minmax(IntCC::UnsignedGreaterThan, a, b)
    }

    pub fn iabs(&mut self, a: Value) -> Value {
        let n = self.ineg(a);
        let c = self.icmp_imm(IntCC::SignedLessThan, a, 0);
        self.select(c, n, a)
    }

    /// c が 0 なら (trapz) / 0 でなければ (trapnz) 止める
    fn trap_if(&mut self, c: Value, on_zero: bool) {
        self.load_rax(c);
        self.emit(&[0x48, 0x85, 0xC0]);
        self.emit(&[if on_zero { 0x75 } else { 0x74 }, 0x02, 0x0F, 0x0B]); // jnz/jz +2; ud2
    }
    /// コードつきの trap (場所を traps() に残す)
    pub fn trap_code(&mut self, code: u8) {
        self.traps.push((self.code.len() as u32, code));
        self.trap();
    }
    fn trap_if_code(&mut self, c: Value, on_zero: bool, code: u8) {
        self.load_rax(c);
        self.emit(&[0x48, 0x85, 0xC0]);
        self.emit(&[if on_zero { 0x75 } else { 0x74 }, 0x02]);
        self.traps.push((self.code.len() as u32, code));
        self.emit(&[0x0F, 0x0B]);
    }
    pub fn trapz_code(&mut self, c: Value, code: u8) {
        self.trap_if_code(c, true, code)
    }
    pub fn trapnz_code(&mut self, c: Value, code: u8) {
        self.trap_if_code(c, false, code)
    }
    /// これまでの trap の (オフセット, コード)
    pub fn traps(&self) -> Vec<(u32, u8)> {
        self.traps.clone()
    }
    pub fn trapz(&mut self, c: Value) {
        self.trap_if(c, true)
    }
    pub fn trapnz(&mut self, c: Value) {
        self.trap_if(c, false)
    }

    // --- 間接呼び出し・アドレス --------------------------------------------------

    /// 関数ポインタ p を呼ぶ
    pub fn call_indirect(&mut self, p: Value, args: &[Value], rets: &[Type]) -> Vec<Value> {
        // (r11 に読んでおき、call の直前に呼ぶ: 引数のレジスタは call() の処理で埋まる)
        let f = self.import_function("\0indirect");
        let n = self.relocs.len();
        let o = self.off(p);
        self.emit_disp(&[0x4C, 0x8B, 0x9D], o); // mov r11, [p]
        let out = self.call(f, args, rets);
        // call() が書いた `call rel32` (E8 + 4) を `call r11` + nop に書き換える
        let r = self.relocs.remove(n);
        let at = r.offset as usize;
        self.code[at - 1..at + 4].copy_from_slice(&[0x41, 0xFF, 0xD3, 0x90, 0x90]);
        out
    }

    /// 名前 (関数かデータ) のアドレス (GOT 経由: 外部の名前でも PIE でも使える)
    pub fn symbol_addr(&mut self, name: &str) -> Value {
        self.emit(&[0x48, 0x8B, 0x05]); // mov rax, [rip + rel32]
        let at = self.code.len() as u32;
        self.emit(&[0, 0, 0, 0]);
        self.relocs.push(Reloc { offset: at, name: name.to_string(), kind: 9, addend: -4 });
        let v = self.new_value(Type::I64);
        self.store_rax(v);
        v
    }
}

// --- ELF64 の再配置可能オブジェクト ------------------------------------------

/// データ (名前, 中身, 再配置 (オフセット, 指す名前, 加数), 外から見えるか)。中のポインタは R_X86_64_64
pub type DataDef = (String, Vec<u8>, Vec<(u32, String, i64)>, bool);

/// 関数 (名前, 機械語, 再配置, 外から見えるか) を .text に並べた ELF の .o
pub fn write_object(funcs: &[(String, Vec<u8>, Vec<Reloc>, bool)]) -> Vec<u8> {
    write_object_full(funcs, &[])
}

/// 関数を .text、データを .data に置いた ELF の .o
pub fn write_object_full(funcs: &[(String, Vec<u8>, Vec<Reloc>, bool)], datas: &[DataDef]) -> Vec<u8> {
    let pad = |v: &mut Vec<u8>, a: usize, fill: u8| {
        while v.len() % a != 0 {
            v.push(fill);
        }
    };
    let mut text: Vec<u8> = vec![];
    let mut offs = vec![];
    for (_, code, _, _) in funcs {
        pad(&mut text, 16, 0xCC);
        offs.push(text.len());
        text.extend_from_slice(code);
    }
    let mut data: Vec<u8> = vec![];
    let mut doffs = vec![];
    for (_, bytes, _, _) in datas {
        pad(&mut data, 16, 0);
        doffs.push(data.len());
        data.extend_from_slice(bytes);
    }
    // 記号: 0 = null、1 = .text、2 = .data のセクション記号、ローカル、グローバル、未定義
    let mut strtab: Vec<u8> = vec![0];
    let add_str = |s: &str, t: &mut Vec<u8>| -> u32 {
        let o = t.len() as u32;
        t.extend_from_slice(s.as_bytes());
        t.push(0);
        o
    };
    // (名前, info, shndx, value, size)
    let mut syms: Vec<(u32, u8, u16, u64, u64)> = vec![(0, 0, 0, 0, 0), (0, 3, 1, 0, 0), (0, 3, 2, 0, 0)];
    let mut index: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for pass in 0..2 {
        let global = pass == 1;
        for (i, (name, code, _, g)) in funcs.iter().enumerate() {
            if *g == global {
                let n = add_str(name, &mut strtab);
                syms.push((n, ((global as u8) << 4) | 2, 1, offs[i] as u64, code.len() as u64));
                index.insert(name.clone(), syms.len() as u32 - 1);
            }
        }
        for (i, (name, bytes, _, g)) in datas.iter().enumerate() {
            if *g == global {
                let n = add_str(name, &mut strtab);
                syms.push((n, ((global as u8) << 4) | 1, 2, doffs[i] as u64, bytes.len() as u64));
                index.insert(name.clone(), syms.len() as u32 - 1);
            }
        }
    }
    let first_global = syms.iter().position(|s| s.1 >> 4 == 1).unwrap_or(syms.len()) as u32;
    let undefined = |name: &str, syms: &mut Vec<(u32, u8, u16, u64, u64)>, index: &mut std::collections::HashMap<String, u32>, strtab: &mut Vec<u8>| {
        if !index.contains_key(name) {
            let n = add_str(name, strtab);
            syms.push((n, 1 << 4, 0, 0, 0));
            index.insert(name.to_string(), syms.len() as u32 - 1);
        }
    };
    for (_, _, rs, _) in funcs {
        for r in rs {
            undefined(&r.name, &mut syms, &mut index, &mut strtab);
        }
    }
    for (_, _, rs, _) in datas {
        for r in rs {
            undefined(&r.1, &mut syms, &mut index, &mut strtab);
        }
    }
    let first_global = first_global.min(syms.len() as u32);
    let mut symtab: Vec<u8> = vec![];
    for &(name, info, shndx, value, size) in &syms {
        symtab.extend_from_slice(&name.to_le_bytes());
        symtab.push(info);
        symtab.push(0);
        symtab.extend_from_slice(&shndx.to_le_bytes());
        symtab.extend_from_slice(&value.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
    }
    let sym_of = |name: &str| -> u64 { index[name] as u64 };
    let rela_entry = |out: &mut Vec<u8>, off: u64, sym: u64, kind: u64, addend: i64| {
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&((sym << 32) | kind).to_le_bytes());
        out.extend_from_slice(&addend.to_le_bytes());
    };
    let mut rela_text: Vec<u8> = vec![];
    for (i, (_, _, rs, _)) in funcs.iter().enumerate() {
        for r in rs {
            rela_entry(&mut rela_text, offs[i] as u64 + r.offset as u64, sym_of(&r.name), r.kind as u64, r.addend);
        }
    }
    let mut rela_data: Vec<u8> = vec![];
    for (i, (_, _, rs, _)) in datas.iter().enumerate() {
        for (o, name, addend) in rs {
            rela_entry(&mut rela_data, doffs[i] as u64 + *o as u64, sym_of(name), 1, *addend);
        }
    }
    // (名前の位置: .text 1, .data 7, .symtab 13, .strtab 21, .rela.text 29, .rela.data 40, .shstrtab 51, .note.GNU-stack 61)
    let shstr = b"\0.text\0.data\0.symtab\0.strtab\0.rela.text\0.rela.data\0.shstrtab\0.note.GNU-stack\0".to_vec();
    let mut out = vec![0u8; 64];
    pad(&mut out, 16, 0);
    let text_off = out.len();
    out.extend_from_slice(&text);
    pad(&mut out, 16, 0);
    let data_off = out.len();
    out.extend_from_slice(&data);
    pad(&mut out, 8, 0);
    let sym_off = out.len();
    out.extend_from_slice(&symtab);
    let str_off = out.len();
    out.extend_from_slice(&strtab);
    pad(&mut out, 8, 0);
    let rt_off = out.len();
    out.extend_from_slice(&rela_text);
    let rd_off = out.len();
    out.extend_from_slice(&rela_data);
    let shstr_off = out.len();
    out.extend_from_slice(&shstr);
    pad(&mut out, 8, 0);
    let sh_off = out.len();
    let sh = |o: &mut Vec<u8>, name: u32, ty: u32, flags: u64, off: usize, size: usize, link: u32, info: u32, al: u64, ent: u64| {
        for x in [name, ty] {
            o.extend_from_slice(&x.to_le_bytes());
        }
        for x in [flags, 0, off as u64, size as u64] {
            o.extend_from_slice(&x.to_le_bytes());
        }
        for x in [link, info] {
            o.extend_from_slice(&x.to_le_bytes());
        }
        for x in [al, ent] {
            o.extend_from_slice(&x.to_le_bytes());
        }
    };
    // 0 null, 1 .text, 2 .data, 3 .symtab, 4 .strtab, 5 .rela.text, 6 .rela.data, 7 .shstrtab, 8 .note.GNU-stack
    sh(&mut out, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    sh(&mut out, 1, 1, 6, text_off, text.len(), 0, 0, 16, 0);
    sh(&mut out, 7, 1, 3, data_off, data.len(), 0, 0, 16, 0);
    sh(&mut out, 13, 2, 0, sym_off, symtab.len(), 4, first_global, 8, 24);
    sh(&mut out, 21, 3, 0, str_off, strtab.len(), 0, 0, 1, 0);
    sh(&mut out, 29, 4, 0x40, rt_off, rela_text.len(), 3, 1, 8, 24);
    sh(&mut out, 40, 4, 0x40, rd_off, rela_data.len(), 3, 2, 8, 24);
    sh(&mut out, 51, 3, 0, shstr_off, shstr.len(), 0, 0, 1, 0);
    sh(&mut out, 61, 1, 0, shstr_off, 0, 0, 0, 1, 0);
    let mut h: Vec<u8> = vec![0x7F, b'E', b'L', b'F', 2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    h.extend_from_slice(&1u16.to_le_bytes()); // ET_REL
    h.extend_from_slice(&62u16.to_le_bytes()); // EM_X86_64
    h.extend_from_slice(&1u32.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes());
    h.extend_from_slice(&0u64.to_le_bytes());
    h.extend_from_slice(&(sh_off as u64).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    for x in [64u16, 0, 0, 64, 9, 7] {
        h.extend_from_slice(&x.to_le_bytes());
    }
    out[..64].copy_from_slice(&h);
    out
}
