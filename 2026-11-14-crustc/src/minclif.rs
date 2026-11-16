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

/// 1 関数分の機械語の組み立て (FunctionBuilder が 2 回使う: 解析 (track) と、割り当てを使った本番)
pub struct Emitter {
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
    /// 直前の命令が「rax を値 v のスロットに書く」なら (v, その後のコードの位置)。
    /// すぐ後で v を rax に読むなら読まなくてよい (ブロックの始まりでは消す: ほかの所から飛んでくる)
    rax_holds: Option<(u32, usize)>,
    /// 今の命令の番号 (FunctionBuilder が記録した順)
    pub op: u32,
    /// 解析のときの記録
    track: Option<Box<Track>>,
    /// 割り当て (本番のとき)
    plan: Plan,
    /// 割り当てたレジスタの保存先
    save_slots: [i32; 5],
}

/// 値・変数を置くレジスタ: rbx, r12, r13, r14, r15 (呼ばれた側が保存するので、呼び出しをまたいでも残る。
/// 命令の組み立てで使う rax / rcx / rdx / 引数のレジスタ / r11 とぶつからない)
/// 後ろの 5 つ (rsi, rdi, r8, r9, r10) は呼ばれた側が保存しないので、呼び出しを含まない生存区間にだけ使う
/// (引数のレジスタは call の中で順に書くが、引数に渡す値の区間はその call を含む)
pub const ALLOC_REGS: [u8; 10] = [3, 12, 13, 14, 15, 6, 7, 8, 9, 10];
/// レジスタに置かない (スロット)
pub const MEM: u8 = 255;
/// 変数の番号を値の番号と分ける
const VAR_BASE: u32 = 1 << 28;

/// 解析の記録: 値・変数 (vreg) の読み書きと、区間 (ブロックを続けて書いた範囲) とその行き先
#[derive(Default)]
pub struct Track {
    /// (vreg, 命令の番号, 書き込みか)
    ev: Vec<(u32, u32, bool)>,
    /// スロットとして直接使う (アドレス・幅の違う読み書き) か、整数でない: レジスタに置けない
    mem_only: Vec<bool>,
    var_mem_only: Vec<bool>,
    /// 浮動小数点か (xmm に置く)
    vfloat: Vec<bool>,
    varfloat: Vec<bool>,
    segs: Vec<Seg>,
    /// 関数を呼ぶ命令の番号
    calls: Vec<u32>,
}

#[derive(Clone, Default)]
struct Seg {
    block: u32,
    start: u32,
    end: u32,
    /// jump / brif の飛び先のブロック
    succ: Vec<u32>,
    /// 終端命令なしで別のブロックへ切り替えた (同じブロックの次の区間へ続く)
    falls: bool,
}

/// 割り当ての結果: 値・変数ごとのレジスタ (MEM ならスロット)、使ったレジスタ (ALLOC_REGS のビット)
#[derive(Clone, Default)]
pub struct Plan {
    pub vloc: Vec<u8>,
    pub varloc: Vec<u8>,
    /// 使った ALLOC_REGS[..5] (呼ばれた側が保存するもの) のビット
    pub used: u8,
}

/// mov dst, src (64 bit。レジスタの番号は 0〜15)
fn mov_rr(dst: u8, src: u8) -> [u8; 3] {
    [0x48 | ((src >> 3) << 2) | (dst >> 3), 0x89, 0xC0 | ((src & 7) << 3) | (dst & 7)]
}

/// movaps xmm_dst, xmm_src
fn movaps(dst: u8, src: u8) -> Vec<u8> {
    let mut o = vec![];
    if dst >= 8 || src >= 8 {
        o.push(0x40 | ((dst >> 3) << 2) | (src >> 3));
    }
    o.extend_from_slice(&[0x0F, 0x28, 0xC0 | ((dst & 7) << 3) | (src & 7)]);
    o
}

/// mov [rbp+d], r / mov r, [rbp+d]
fn mov_mr(r: u8, d: i32, load: bool) -> [u8; 7] {
    let db = d.to_le_bytes();
    [0x48 | ((r >> 3) << 2), if load { 0x8B } else { 0x89 }, 0x85 | ((r & 7) << 3), db[0], db[1], db[2], db[3]]
}

struct BlockData {
    params: Vec<Value>,
    /// コードの位置 (切り替えたとき)
    offset: Option<usize>,
    /// 終端命令の前に別のブロックへ切り替えたとき、続きへ飛ぶ jmp の rel32 の位置 (戻ってきたら今の位置へ向ける)
    cont: Option<usize>,
}

impl Emitter {
    pub fn new(params: Vec<Type>, rets: Vec<Type>, plan: Option<Plan>) -> Emitter {
        let track = if plan.is_none() {
            // (入口の前の区間: 最初のブロックへ続く)
            let mut t = Track::default();
            t.segs.push(Seg { block: u32::MAX, start: 0, end: 0, succ: vec![], falls: true });
            Some(Box::new(t))
        } else {
            None
        };
        let plan = plan.unwrap_or_default();
        let mut b = Emitter {
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
            rax_holds: None,
            op: 0,
            track,
            plan,
            save_slots: [0; 5],
        };
        // プロローグ: push rbp; mov rbp, rsp; sub rsp, <frame> (frame は最後に書き込む)
        b.code.extend_from_slice(&[0x55, 0x48, 0x89, 0xE5, 0x48, 0x81, 0xEC, 0, 0, 0, 0]);
        // (割り当てたレジスタの保存)
        for i in 0..5 {
            b.save_slots[i] = b.alloc(8, 8);
            if b.plan.used & (1 << i) != 0 {
                let d = b.save_slots[i];
                b.code.extend_from_slice(&mov_mr(ALLOC_REGS[i], d, false));
            }
        }
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
                b.force_mem(v);
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
            // (整数の引数はレジスタに置ける: 入口 (命令 0) で書いたことにし、本番では下でレジスタへ読む)
            if t == Type::I128 {
                b.force_mem(v);
            } else {
                b.ev_def(v.0);
            }
        }
        // (入口で 0 にしたいスロットがあれば finish_zeroing がここから飛ぶ: 普段は次の命令へ飛ぶだけ)
        for k in 0..b.entry_params.len() {
            let v = b.entry_params[k];
            let r = b.loc(v);
            if r != MEM {
                let o = b.values[v.0 as usize].0;
                b.code.extend_from_slice(&mov_mr(r, o, true));
            }
        }
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
        if let Some(tr) = &mut self.track {
            tr.mem_only.push(t == Type::I128);
            tr.vfloat.push(t.is_float());
        }
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
        let falls = self.cur.is_some_and(|c| c != b) && !self.terminated;
        if self.cur != Some(b) {
            self.seg_switch(b, falls);
        }
        if let Some(c) = self.cur {
            if !self.terminated && c != b {
                self.emit(&[0xE9]);
                let at = self.code.len();
                self.emit(&[0, 0, 0, 0]);
                self.blocks[c.0 as usize].cont = Some(at);
            }
        }
        let here = self.code.len();
        self.rax_holds = None;
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
        if let Some(tr) = &mut self.track {
            tr.var_mem_only.push(t == Type::I128);
            tr.varfloat.push(t.is_float());
        }
        Variable(self.vars.len() as u32 - 1)
    }

    pub fn def_var(&mut self, var: Variable, v: Value) {
        let (off, t) = self.vars[var.0 as usize];
        // (両方レジスタなら直接写す)
        let (lv, lr) = (self.loc(v), self.var_loc(var));
        if lv != MEM && lr != MEM && (lv >= 16) == (lr >= 16) {
            self.ev_use(v.0);
            self.ev_def(VAR_BASE + var.0);
            if lv >= 16 {
                self.emit(&movaps(lr - 16, lv - 16));
            } else {
                self.emit(&mov_rr(lr, lv));
            }
            self.rax_holds = None;
            return;
        }
        self.load_rax(v);
        self.ev_def(VAR_BASE + var.0);
        let l = self.var_loc(var);
        if l != MEM && l >= 16 {
            // movq xmm, rax
            let k = l - 16;
            self.emit(&[0x66, 0x48 | ((k >> 3) << 2), 0x0F, 0x6E, 0xC0 | ((k & 7) << 3)]);
        } else if l != MEM {
            self.emit(&mov_rr(l, 0));
        } else {
            self.store_rax_at(off);
        }
        if t == Type::I128 {
            let o = self.off(v);
            self.load_rax_at(o + 8);
            self.store_rax_at(off + 8);
        }
    }

    pub fn use_var(&mut self, var: Variable) -> Value {
        let (off, t) = self.vars[var.0 as usize];
        let (lr, ld) = (self.var_loc(var), self.next_loc());
        if lr != MEM && ld != MEM && (lr >= 16) == (ld >= 16) {
            self.ev_use(VAR_BASE + var.0);
            if lr >= 16 {
                self.emit(&movaps(ld - 16, lr - 16));
            } else {
                self.emit(&mov_rr(ld, lr));
            }
            return self.result_in_reg(t);
        }
        let v = self.new_value(t);
        self.ev_use(VAR_BASE + var.0);
        let l = self.var_loc(var);
        if l != MEM && l >= 16 {
            // movq rax, xmm
            let k = l - 16;
            self.emit(&[0x66, 0x48 | ((k >> 3) << 2), 0x0F, 0x7E, 0xC0 | ((k & 7) << 3)]);
        } else if l != MEM {
            self.emit(&mov_rr(0, l));
        } else {
            self.load_rax_at(off);
        }
        if t == Type::I128 {
            self.force_mem(v);
            let o = self.values[v.0 as usize].0;
            self.store_rax_at(o);
            self.load_rax_at(off + 8);
            self.store_rax_at(o + 8);
        } else {
            self.store_rax(v);
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

    /// rax が今 v を持っているか (直前に書いたばかり)
    fn rax_is(&self, v: Value) -> bool {
        self.rax_holds == Some((v.0, self.code.len()))
    }

    /// 値 v を整数のレジスタ r に読む
    fn load_reg(&mut self, r: u8, v: Value) {
        self.ev_use(v.0);
        let l = self.loc(v);
        if l != MEM && l >= 16 {
            // movq r64, xmm (f32 は movd r32, xmm: 上位は 0)
            let k = l - 16;
            let w = if self.values[v.0 as usize].1 == Type::F64 { 0x48 } else { 0x40 };
            self.emit(&[0x66, w | ((k >> 3) << 2) | (r >> 3), 0x0F, 0x7E, 0xC0 | ((k & 7) << 3) | (r & 7)]);
            return;
        }
        if l != MEM {
            if l != r {
                self.emit(&mov_rr(r, l));
            }
            return;
        }
        let off = self.values[v.0 as usize].0;
        self.emit(&mov_mr(r, off, true));
    }

    fn load_rax(&mut self, v: Value) {
        if self.rax_is(v) {
            self.ev_use(v.0);
            return;
        }
        self.load_reg(0, v);
    }

    fn load_rcx(&mut self, v: Value) {
        if self.rax_is(v) {
            self.ev_use(v.0);
            self.emit(&[0x48, 0x89, 0xC1]); // mov rcx, rax
            return;
        }
        self.load_reg(1, v);
    }

    fn store_rax(&mut self, v: Value) {
        self.ev_def(v.0);
        let l = self.loc(v);
        if l != MEM && l >= 16 {
            // movq xmm, rax (f32 は movd xmm, eax)
            let k = l - 16;
            let w = if self.values[v.0 as usize].1 == Type::F64 { 0x48 } else { 0x40 };
            self.emit(&[0x66, w | ((k >> 3) << 2), 0x0F, 0x6E, 0xC0 | ((k & 7) << 3)]);
            self.rax_holds = Some((v.0, self.code.len()));
            return;
        }
        if l != MEM {
            self.emit(&mov_rr(l, 0));
        } else {
            let off = self.values[v.0 as usize].0;
            self.store_rax_at(off);
        }
        if self.values[v.0 as usize].1 != Type::I128 {
            self.rax_holds = Some((v.0, self.code.len()));
        }
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
        // (結果をレジスタに置くなら直接書く: 型の幅で 0 拡張した値)
        let d = self.next_loc();
        if d != MEM && t != Type::I128 && !t.is_float() {
            let xv: u64 = match t {
                Type::I8 => x as u64 & 0xFF,
                Type::I16 => x as u64 & 0xFFFF,
                Type::I32 => x as u64 & 0xFFFF_FFFF,
                _ => x as u64,
            };
            let b = if d >= 8 { 1u8 } else { 0 };
            if xv <= 0xFFFF_FFFF {
                // mov r32, imm32 (上位は 0 になる)
                if b != 0 {
                    self.emit(&[0x41]);
                }
                self.emit(&[0xB8 + (d & 7)]);
                self.emit(&(xv as u32).to_le_bytes());
            } else if (xv as i64) >= i32::MIN as i64 && (xv as i64) < 0 {
                // mov r64, imm32 (符号拡張)
                self.emit(&[0x48 | b, 0xC7, 0xC0 | (d & 7)]);
                self.emit(&(xv as i64 as i32).to_le_bytes());
            } else {
                self.emit(&[0x48 | b, 0xB8 + (d & 7)]);
                self.emit(&xv.to_le_bytes());
            }
            return self.result_in_reg(t);
        }
        self.emit(&[0x48, 0xB8]);
        self.emit(&x.to_le_bytes());
        self.result(t)
    }

    /// 64 / 32 bit の a と b がレジスタなら cmp a, b を書いて true (32 bit の比較は符号つきも正しい)
    fn cmp_regs(&mut self, t: Type, a: Value, b: Value) -> bool {
        if t != Type::I64 && t != Type::I32 {
            return false;
        }
        let (Some(ra), Some(rb)) = (self.rloc(a), self.rloc(b)) else { return false };
        let w = if t == Type::I64 { 0x48 } else { 0x40 };
        // cmp ra, rb: 39 /r (reg = rb, rm = ra)
        self.emit(&[w | ((rb >> 3) << 2) | (ra >> 3), 0x39, 0xC0 | ((rb & 7) << 3) | (ra & 7)]);
        true
    }

    /// modrm (+ SIB) + disp32: [base + off]、reg フィールドは r
    fn mem_operand(&mut self, r: u8, base: u8, off: i32) {
        self.emit(&[0x80 | ((r & 7) << 3) | (base & 7)]);
        if base & 7 == 4 {
            self.emit(&[0x24]);
        }
        self.emit(&off.to_le_bytes());
    }

    /// 次に作る値のレジスタ (MEM ならスロット)
    fn next_loc(&self) -> u8 {
        self.plan.vloc.get(self.values.len()).copied().unwrap_or(MEM)
    }

    /// 次の値をレジスタに直接書いた後: 値を作る (rax は持っていない)
    fn result_in_reg(&mut self, t: Type) -> Value {
        let v = self.new_value(t);
        self.ev_def(v.0);
        self.rax_holds = None;
        v
    }

    /// 値のレジスタ (スロットなら None)
    fn rloc(&self, v: Value) -> Option<u8> {
        let l = self.loc(v);
        if l == MEM { None } else { Some(l) }
    }

    fn binop(&mut self, a: Value, b: Value, op: &[u8]) -> Value {
        let t = self.value_type(a);
        // (64 / 32 bit で、両方と結果がレジスタなら 2 オペランドの形: mov d, a; op d, b。
        //  生存区間は閉じているので d は a / b のレジスタと違う。32 bit の命令は上位を 0 にする)
        if t == Type::I64 || t == Type::I32 {
            let d = self.next_loc();
            if let (Some(ra), Some(rb), true) = (self.rloc(a), self.rloc(b), d != MEM) {
                // (add / sub / and / or / xor rax, rcx と imul rax, rcx)
                let code: Option<u8> = if op.len() == 3 && op[2] == 0xC8 && matches!(op[1], 0x01 | 0x29 | 0x21 | 0x09 | 0x31) {
                    Some(op[1])
                } else if op.len() == 4 && op[1] == 0x0F && op[2] == 0xAF {
                    Some(0xAF)
                } else {
                    None
                };
                if let Some(c) = code {
                    let w = if t == Type::I64 { 0x48 } else { 0x40 };
                    self.emit(&mov_rr(d, ra));
                    if c == 0xAF {
                        // imul d, b: 0F AF /r (reg = d)
                        self.emit(&[w | ((d >> 3) << 2) | (rb >> 3), 0x0F, 0xAF, 0xC0 | ((d & 7) << 3) | (rb & 7)]);
                    } else {
                        // op d, b: op /r (reg = b, rm = d)
                        self.emit(&[w | ((rb >> 3) << 2) | (d >> 3), c, 0xC0 | ((rb & 7) << 3) | (d & 7)]);
                    }
                    return self.result_in_reg(t);
                }
            }
        }
        // (rax が b を持っていれば先に rcx へ移す)
        if self.rax_is(b) && a != b {
            self.load_rcx(b);
            self.load_rax(a);
        } else {
            self.load_rax(a);
            self.load_rcx(b);
        }
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
        if self.cmp_regs(t, a, b) {
            self.emit(&[0x0F, cc.setcc(), 0xC0]); // setcc al
            return self.result(Type::I8);
        }
        self.load_rax(a);
        self.load_rcx(b);
        if cc.signed() {
            self.sext_rax(t);
            self.sext_rcx(t);
        }
        self.emit(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
        self.emit(&[0x0F, cc.setcc(), 0xC0]); // setcc al
        // (movzx rax, al は result がする)
        self.result(Type::I8)
    }

    /// `brif(icmp(cc, a, b), ..)` で icmp の値をほかで使わないとき: 値を作らずに cmp と jcc で分ける。
    /// (icmp の値の番号は同じにする: 作るだけで書かない)
    pub fn cmp_only(&mut self, cc: IntCC, a: Value, b: Value) {
        let t = self.value_type(a);
        if self.cmp_regs(t, a, b) {
            let _ = self.new_value(Type::I8);
            self.rax_holds = None;
            return;
        }
        self.load_rax(a);
        self.load_rcx(b);
        if cc.signed() {
            self.sext_rax(t);
            self.sext_rcx(t);
        }
        self.emit(&[0x48, 0x39, 0xC8]); // cmp rax, rcx
        let _ = self.new_value(Type::I8);
        self.rax_holds = None;
    }

    /// cmp_only の後: 成り立たなければ else へ (setcc の逆の条件の jcc)
    pub fn brif_cc(&mut self, cc: IntCC, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        self.branch_on((cc.setcc() ^ 1) - 0x10, then_b, then_args, else_b, else_args);
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
        let off = self.off(c);
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
        // (アドレスと結果がレジスタなら mov d, [base + off] / movzx)
        let d = self.next_loc();
        if let (Some(base), true) = (self.rloc(addr), d != MEM && matches!(t, Type::I64 | Type::I32 | Type::I16 | Type::I8)) {
            let rx = ((d >> 3) << 2) | (base >> 3);
            match t {
                Type::I64 => self.emit(&[0x48 | rx, 0x8B]),
                Type::I32 => self.emit(&[0x40 | rx, 0x8B]),
                Type::I16 => self.emit(&[0x48 | rx, 0x0F, 0xB7]),
                _ => self.emit(&[0x48 | rx, 0x0F, 0xB6]),
            }
            self.mem_operand(d, base, off);
            return self.result_in_reg(t);
        }
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
                let o = self.off(v);
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
        // (値とアドレスがレジスタなら mov [base + off], r)
        if let (Some(r), Some(base)) = (self.rloc(v), self.rloc(addr)) {
            if matches!(t, Type::I64 | Type::I32 | Type::I16 | Type::I8) {
                let rx = ((r >> 3) << 2) | (base >> 3);
                match t {
                    Type::I64 => self.emit(&[0x48 | rx, 0x89]),
                    Type::I32 => self.emit(&[0x40 | rx, 0x89]),
                    Type::I16 => self.emit(&[0x66, 0x40 | rx, 0x89]),
                    // (REX を付けると sil / dil などの下位 8 bit になる)
                    _ => self.emit(&[0x40 | rx, 0x88]),
                }
                self.mem_operand(r, base, off);
                return;
            }
        }
        if self.rax_is(v) && v != addr {
            self.load_rcx(v);
            self.load_rax(addr);
        } else {
            self.load_rax(addr);
            self.load_rcx(v);
        }
        match t {
            Type::I64 | Type::F64 => self.emit_disp(&[0x48, 0x89, 0x88], off),
            Type::I32 | Type::F32 => self.emit_disp(&[0x89, 0x88], off),
            Type::I16 => self.emit_disp(&[0x66, 0x89, 0x88], off),
            Type::I8 => self.emit_disp(&[0x88, 0x88], off),
            Type::I128 => {
                self.emit_disp(&[0x48, 0x89, 0x88], off);
                let o = self.off(v);
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
        // (1 つだけなら入れ替わりは起きない: そのまま書く)
        if args.len() == 1 && ps.len() >= 1 && self.value_type(args[0]) != Type::I128 {
            self.load_rax(args[0]);
            self.store_rax(ps[0]);
            return;
        }
        // (引数どうしが入れ替わる場合に備え、全部読んでから書く: スタックに積む)
        for &a in args {
            self.load_rax(a);
            self.emit(&[0x50]); // push rax
            if self.value_type(a) == Type::I128 {
                let o = self.off(a);
                self.load_rax_at(o + 8);
                self.emit(&[0x50]);
            }
        }
        for &p in ps.iter().rev().take(args.len()) {
            if self.value_type(p) == Type::I128 {
                let o = self.off(p);
                self.emit(&[0x58]);
                self.store_rax_at(o + 8);
            }
            self.emit(&[0x58]); // pop rax
            self.store_rax(p);
        }
    }

    pub fn jump(&mut self, b: Block, args: &[Value]) {
        self.seg_succ(b);
        self.pass_args(b, args);
        self.emit(&[0xE9]);
        self.rel32_to(b);
        self.terminated = true;
    }

    /// c が 0 でなければ then_b へ、0 なら else_b へ
    pub fn brif(&mut self, c: Value, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        self.load_rax(c);
        self.emit(&[0x48, 0x85, 0xC0]); // test rax, rax
        self.branch_on(0x84, then_b, then_args, else_b, else_args);
    }

    /// 直前の比べた結果で、条件 jcc (0x0F の次の byte) が成り立てば else へ、成り立たなければ then へ
    fn branch_on(&mut self, jcc: u8, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        if else_args.is_empty() {
            // (else に渡すものがなければ直接飛ぶ)
            self.seg_succ(else_b);
            self.emit(&[0x0F, jcc]);
            self.rel32_to(else_b);
            self.jump(then_b, then_args);
            return;
        }
        self.emit(&[0x0F, jcc]);
        let jz_at = self.code.len();
        self.emit(&[0, 0, 0, 0]);
        self.jump(then_b, then_args);
        let here = self.code.len() as i32;
        let rel = here - (jz_at as i32 + 4);
        self.code[jz_at..jz_at + 4].copy_from_slice(&rel.to_le_bytes());
        self.rax_holds = None;
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
                let off = self.off(v);
                self.emit_disp(&[0x48, 0x8B, 0x95], off + 8);
            }
        }
        if let Some(&v) = vals.get(1) {
            // 2 つ目は rdx
            let off = self.off(v);
            self.emit_disp(&[0x48, 0x8B, 0x95], off);
        }
        // (割り当てたレジスタを戻す)
        for i in 0..5 {
            if self.plan.used & (1 << i) != 0 {
                let d = self.save_slots[i];
                self.emit(&mov_mr(ALLOC_REGS[i], d, true));
            }
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
        let op = self.op;
        if let Some(tr) = &mut self.track {
            tr.calls.push(op);
        }
        const ARG_LOAD: [[u8; 3]; 6] = [[0x48, 0x8B, 0xBD], [0x48, 0x8B, 0xB5], [0x48, 0x8B, 0x95], [0x48, 0x8B, 0x8D], [0x4C, 0x8B, 0x85], [0x4C, 0x8B, 0x8D]];
        let fargs: Vec<Value> = args.iter().copied().filter(|&a| self.value_type(a).is_float()).collect();
        let args: Vec<Value> = args.iter().copied().filter(|&a| !self.value_type(a).is_float()).collect();
        // 8 byte ずつの部分 (スロットのオフセット) をレジスタとスタックに分ける (i128 は 2 つ。2 つ残っていなければ全体をスタック)
        // (レジスタで渡す部分は (値, 上位か)。写しがあればそこから読む)
        let mut regs: Vec<(Value, bool)> = vec![];
        let mut stack_parts: Vec<i32> = vec![];
        for &a in &args {
            let wide = self.value_type(a) == Type::I128;
            let n = if wide { 2 } else { 1 };
            if regs.len() + n <= 6 && stack_parts.is_empty() {
                regs.push((a, false));
                if wide {
                    regs.push((a, true));
                }
            } else {
                let off = self.off(a);
                stack_parts.push(off);
                if wide {
                    stack_parts.push(off + 8);
                }
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
        const ARG_REG: [u8; 6] = [7, 6, 2, 1, 8, 9];
        // (引数の値が引数のレジスタにあると、順に書くと先に書いたもので壊れる: そのときはスタックを経由する)
        let clash = regs.iter().any(|&(v, hi)| !hi && self.value_type(v) != Type::I128 && ARG_REG.contains(&self.loc(v)));
        if clash {
            for &(v, hi) in regs.iter() {
                if hi || self.value_type(v) == Type::I128 {
                    let off = self.off(v) + if hi { 8 } else { 0 };
                    self.load_rax_at(off);
                } else {
                    self.load_reg(0, v);
                }
                self.emit(&[0x50]); // push rax
            }
            for i in (0..regs.len()).rev() {
                let r = ARG_REG[i];
                // pop r
                if r >= 8 {
                    self.emit(&[0x41, 0x58 + (r - 8)]);
                } else {
                    self.emit(&[0x58 + r]);
                }
            }
            self.rax_holds = None;
        } else {
            for (i, &(v, hi)) in regs.iter().enumerate() {
                if hi || self.value_type(v) == Type::I128 {
                    let off = self.off(v) + if hi { 8 } else { 0 };
                    self.emit_disp(&ARG_LOAD[i], off);
                } else {
                    self.load_reg(ARG_REG[i], v);
                }
            }
        }
        for (k, &a) in fargs.iter().enumerate().take(8) {
            self.xmm_load(k as u8, a);
        }
        // (可変長引数の関数のための al = xmm で渡した数。自分の関数 (`_x..`) は可変長引数でないので要らない)
        if !self.funcs[f.0 as usize].starts_with("_x") {
            self.emit(&[0xB8]);
            self.emit(&(fargs.len().min(8) as u32).to_le_bytes());
        }
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
                let o = self.off(v);
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
            // (すぐ次への jmp は 5 byte の nop に)
            if rel == 0 && at >= 1 && self.code[at - 1] == 0xE9 {
                self.code[at - 1..at + 4].copy_from_slice(&[0x0F, 0x1F, 0x44, 0x00, 0x00]);
            }
        }
        let _ = (&self.params, &self.rets);
        (self.code, self.relocs)
    }
}


// --- 浮動小数点・i128・オーバーフロー・ビット演算・間接呼び出し・アドレス ---------------
impl Emitter {
    /// 値のスロットのオフセット (メモリから読む・アドレスとして使う: そのスロットへの書き込みは消せない)
    fn off(&mut self, v: Value) -> i32 {
        self.ev_use(v.0);
        self.force_mem(v);
        // (レジスタに置いた値はスロットにない: 念のため書いてから。解析で mem_only にしたので普通は起きない)
        let r = self.loc(v);
        let o = self.values[v.0 as usize].0;
        if r != MEM && r >= 16 {
            // movsd [rbp+o], xmm
            let k = r - 16;
            self.emit(&[0xF2, 0x40 | ((k >> 3) << 2), 0x0F, 0x11, 0x85 | ((k & 7) << 3)]);
            self.emit(&o.to_le_bytes());
        } else if r != MEM {
            self.emit(&mov_mr(r, o, false));
        }
        o
    }

    /// 値のレジスタ (MEM ならスロット)
    fn loc(&self, v: Value) -> u8 {
        self.plan.vloc.get(v.0 as usize).copied().unwrap_or(MEM)
    }
    fn var_loc(&self, var: Variable) -> u8 {
        self.plan.varloc.get(var.0 as usize).copied().unwrap_or(MEM)
    }
    fn force_mem(&mut self, v: Value) {
        if let Some(tr) = &mut self.track {
            tr.mem_only[v.0 as usize] = true;
        }
    }
    fn ev_use(&mut self, id: u32) {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            tr.ev.push((id, op, false));
        }
    }
    fn ev_def(&mut self, id: u32) {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            tr.ev.push((id, op, true));
        }
    }
    /// 区間を閉じて次を始める (解析のとき)
    fn seg_switch(&mut self, b: Block, falls: bool) {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            if let Some(last) = tr.segs.last_mut() {
                last.end = op;
                last.falls = falls;
            }
            tr.segs.push(Seg { block: b.0, start: op, end: op, succ: vec![], falls: false });
        }
    }
    fn seg_succ(&mut self, b: Block) {
        if let Some(tr) = &mut self.track {
            if let Some(last) = tr.segs.last_mut() {
                last.succ.push(b.0);
            }
        }
    }
    pub fn take_track(&mut self) -> Option<Box<Track>> {
        let op = self.op;
        if let Some(tr) = &mut self.track {
            if let Some(last) = tr.segs.last_mut() {
                last.end = op;
            }
        }
        self.track.take()
    }


    /// xmm0 (reg 0) / xmm1 (reg 1) に読む
    fn xmm_load(&mut self, reg: u8, v: Value) {
        let p = if self.value_type(v) == Type::F32 { 0xF3 } else { 0xF2 };
        // (xmm に置いた値: movaps xmm_reg, xmm_k)
        let l = self.loc(v);
        if l != MEM && l >= 16 {
            self.ev_use(v.0);
            let k = l - 16;
            self.emit(&movaps(reg, k));
            return;
        }
        // (スロットから: レジスタに置けなくする off ではなく、読んだことだけ記録する)
        self.ev_use(v.0);
        let o = self.values[v.0 as usize].0;
        self.emit_disp(&[p, 0x0F, 0x10, 0x85 | (reg << 3)], o);
    }

    fn xmm0_result(&mut self, t: Type) -> Value {
        let d = self.next_loc();
        if d != MEM && d >= 16 {
            self.emit(&movaps(d - 16, 0));
            return self.result_in_reg(t);
        }
        let v = self.new_value(t);
        self.ev_def(v.0);
        let p = if t == Type::F32 { 0xF3 } else { 0xF2 };
        let o = self.values[v.0 as usize].0;
        // (スロットの上位を 0 にしてから書く: f32 も 8 byte のスロットで比べられるように)
        self.emit(&[0x31, 0xC0]);
        self.store_rax_at(o);
        self.emit_disp(&[p, 0x0F, 0x11, 0x85], o);
        self.rax_holds = None;
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
        let l = self.loc(b);
        if l != MEM && l >= 16 {
            // op xmm0, xmm_k
            let k = l - 16;
            self.ev_use(b.0);
            if k >= 8 {
                self.emit(&[p, 0x41, 0x0F, opc, 0xC0 | (k & 7)]);
            } else {
                self.emit(&[p, 0x0F, opc, 0xC0 | k]);
            }
        } else {
            self.ev_use(b.0);
            let o = self.values[b.0 as usize].0;
            self.emit_disp(&[p, 0x0F, opc, 0x85], o);
        }
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
        if !t.is_float() {
            // (整数にするときは幅で 0 拡張する: f32 のスロットの上位 4 byte は決まっていない)
            return self.result(t);
        }
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

// --- レジスタ割り当て -------------------------------------------------------------

/// 解析の記録から、値・変数をレジスタに割り当てる (線形走査)。
/// 生存区間は命令の番号の範囲 [最初, 最後]: 1 つの区間 (ブロックを続けて書いた範囲) の中だけで書いて読む値はそのまま、
/// 区間をまたぐものは区間の入口・出口で生きているか (後ろ向きのデータフロー) を見て、生きている区間の端まで延ばす
pub fn plan(tr: &Track, nvals: usize, nvars: usize) -> Plan {
    let n = nvals + nvars;
    let idx = |id: u32| -> usize { if id >= VAR_BASE { nvals + (id - VAR_BASE) as usize } else { id as usize } };
    let cand = |i: usize| -> bool { if i < nvals { !tr.mem_only.get(i).copied().unwrap_or(true) } else { !tr.var_mem_only.get(i - nvals).copied().unwrap_or(true) } };
    let segs = &tr.segs;
    // 命令の番号 → 区間 (区間は始まりの順)
    let seg_of = |op: u32| -> usize {
        let mut lo = 0usize;
        let mut hi = segs.len();
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
    const NONE: u32 = u32::MAX;
    let mut first = vec![NONE; n];
    let mut last = vec![0u32; n];
    let mut first_def = vec![false; n];
    let mut seg1 = vec![usize::MAX; n];
    let mut multi = vec![false; n];
    for &(id, op, def) in &tr.ev {
        let i = idx(id);
        if i >= n {
            continue;
        }
        let s = seg_of(op);
        if first[i] == NONE {
            first[i] = op;
            first_def[i] = def;
            seg1[i] = s;
        } else if seg1[i] != s {
            multi[i] = true;
        }
        if op > last[i] {
            last[i] = op;
        }
    }
    // 区間をまたぐもの (と、区間の中で書く前に読むもの): データフローで見る
    let mut gid = vec![usize::MAX; n];
    let mut globals: Vec<usize> = vec![];
    for i in 0..n {
        if first[i] != NONE && cand(i) && (multi[i] || !first_def[i]) {
            gid[i] = globals.len();
            globals.push(i);
        }
    }
    let mut lo: Vec<u32> = first.clone();
    let mut hi: Vec<u32> = last.clone();
    if !globals.is_empty() && !segs.is_empty() {
        let g = globals.len();
        let words = (g + 63) / 64;
        let ns = segs.len();
        let mut gens = vec![0u64; ns * words];
        let mut kill = vec![0u64; ns * words];
        for &(id, op, def) in &tr.ev {
            let i = idx(id);
            if i >= n || gid[i] == usize::MAX {
                continue;
            }
            let s = seg_of(op);
            let (w, bit) = (gid[i] / 64, 1u64 << (gid[i] % 64));
            if def {
                kill[s * words + w] |= bit;
            } else if kill[s * words + w] & bit == 0 {
                gens[s * words + w] |= bit;
            }
        }
        // 区間の行き先: 飛び先のブロックの最初の区間、続き (同じブロックの次の区間 / 入口の前の区間は次)
        let mut first_seg: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
        for (k, sg) in segs.iter().enumerate() {
            first_seg.entry(sg.block).or_insert(k);
        }
        let mut succ: Vec<Vec<usize>> = vec![vec![]; ns];
        for (k, sg) in segs.iter().enumerate() {
            for b in &sg.succ {
                if let Some(&t) = first_seg.get(b) {
                    succ[k].push(t);
                }
            }
            if sg.block == u32::MAX && k + 1 < ns {
                succ[k].push(k + 1);
            } else if sg.falls {
                if let Some(t) = (k + 1..ns).find(|&t| segs[t].block == sg.block) {
                    succ[k].push(t);
                }
            }
        }
        let mut lin = vec![0u64; ns * words];
        let mut lout = vec![0u64; ns * words];
        loop {
            let mut changed = false;
            for k in (0..ns).rev() {
                for w in 0..words {
                    let mut o = 0u64;
                    for &t in &succ[k] {
                        o |= lin[t * words + w];
                    }
                    let iv = gens[k * words + w] | (o & !kill[k * words + w]);
                    if o != lout[k * words + w] || iv != lin[k * words + w] {
                        lout[k * words + w] = o;
                        lin[k * words + w] = iv;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (gi, &i) in globals.iter().enumerate() {
            let (w, bit) = (gi / 64, 1u64 << (gi % 64));
            for k in 0..ns {
                if lin[k * words + w] & bit != 0 && segs[k].start < lo[i] {
                    lo[i] = segs[k].start;
                }
                if lout[k * words + w] & bit != 0 && segs[k].end > hi[i] {
                    hi[i] = segs[k].end;
                }
            }
        }
    }
    // 線形走査: 始まりの順に、空いているレジスタへ。呼び出しを含む区間は呼ばれた側が保存するレジスタだけ。
    // なければ「使う回数 / 長さ」がいちばん小さいもの (今のものか、同じ種類のレジスタを持っているもの) をスロットへ
    let mut uses = vec![0u32; n];
    for &(id, _, _) in &tr.ev {
        let i = idx(id);
        if i < n {
            uses[i] += 1;
        }
    }
    let mut calls = tr.calls.clone();
    calls.sort_unstable();
    let has_call = |a: u32, b: u32| -> bool {
        // (a 以上の最初の呼び出し: 二分探索)
        let (mut l, mut h) = (0usize, calls.len());
        while l < h {
            let m = (l + h) / 2;
            if calls[m] < a {
                l = m + 1;
            } else {
                h = m;
            }
        }
        l < calls.len() && calls[l] <= b
    };
    let weight = |i: usize| -> f64 { uses[i] as f64 / (hi[i] - lo[i] + 1) as f64 };
    let mut order: Vec<usize> = (0..n).filter(|&i| first[i] != NONE && cand(i)).collect();
    order.sort_by_key(|&i| (lo[i], i));
    let mut loc = vec![MEM; n];
    let mut active: Vec<usize> = vec![];
    let mut free_saved: Vec<u8> = vec![4, 3, 2, 1, 0];
    let mut free_temp: Vec<u8> = vec![9, 8, 7, 6, 5];
    // (浮動小数点: xmm8〜15。どれも呼び出しで壊れるので、呼び出しを含まない区間だけ。番号は 16 + xmm の番号)
    let mut free_xmm: Vec<u8> = vec![31, 30, 29, 28, 27, 26, 25, 24];
    let is_float = |i: usize| -> bool { if i < nvals { tr.vfloat.get(i).copied().unwrap_or(false) } else { tr.varfloat.get(i - nvals).copied().unwrap_or(false) } };
    let mut used = 0u8;
    for &i in &order {
        // (終わったものを外す)
        let mut k = 0;
        while k < active.len() {
            let a = active[k];
            if hi[a] < lo[i] {
                if loc[a] >= 16 {
                    free_xmm.push(loc[a]);
                } else if loc[a] < 5 {
                    free_saved.push(loc[a]);
                } else {
                    free_temp.push(loc[a]);
                }
                active.remove(k);
            } else {
                k += 1;
            }
        }
        let crosses = has_call(lo[i], hi[i]);
        if is_float(i) {
            if !crosses {
                if let Some(r) = free_xmm.pop() {
                    loc[i] = r;
                    active.push(i);
                }
            }
            continue;
        }
        let got = if crosses { free_saved.pop() } else { free_temp.pop().or_else(|| free_saved.pop()) };
        if let Some(r) = got {
            loc[i] = r;
            if r < 5 {
                used |= 1 << r;
            }
            active.push(i);
            continue;
        }
        // (追い出す: 呼び出しをまたぐなら、呼ばれた側が保存するレジスタを持つものから)
        let mut worst: Option<usize> = None;
        for k in 0..active.len() {
            let a = active[k];
            if (crosses && loc[a] >= 5) || loc[a] >= 16 {
                continue;
            }
            let better = match worst {
                None => true,
                Some(w) => weight(a) < weight(active[w]),
            };
            if better {
                worst = Some(k);
            }
        }
        if let Some(w) = worst {
            let a = active[w];
            if weight(a) < weight(i) {
                loc[i] = loc[a];
                loc[a] = MEM;
                active[w] = i;
            }
        }
    }
    // (番号 → レジスタの番号)
    let reg = |l: u8| -> u8 {
        if l == MEM {
            MEM
        } else if l >= 16 {
            l
        } else {
            ALLOC_REGS[l as usize]
        }
    };
    Plan { vloc: (0..nvals).map(|i| reg(loc[i])).collect(), varloc: (nvals..n).map(|i| reg(loc[i])).collect(), used }
}

// --- 記録して 2 回組み立てる FunctionBuilder ---------------------------------------------

/// FunctionBuilder の命令 (呼ばれた順に記録し、割り当てを決めてから本番の Emitter でもう一度呼ぶ)
#[derive(Clone)]
enum Op {
    CreateBlock,
    AppendBlockParam(Block, Type),
    SwitchToBlock(Block),
    DeclareVar(Type),
    DefVar(Variable, Value),
    UseVar(Variable),
    CreateStackSlot(u32, u32),
    ImportFunction(String),
    Iconst(Type, i64),
    Iadd(Value, Value),
    Isub(Value, Value),
    Imul(Value, Value),
    Band(Value, Value),
    Bor(Value, Value),
    Bxor(Value, Value),
    IaddImm(Value, i64),
    Udiv(Value, Value),
    Sdiv(Value, Value),
    Urem(Value, Value),
    Srem(Value, Value),
    Ishl(Value, Value),
    Ushr(Value, Value),
    Sshr(Value, Value),
    Ineg(Value),
    Bnot(Value),
    Icmp(IntCC, Value, Value),
    IcmpImm(IntCC, Value, i64),
    Select(Value, Value, Value),
    Uextend(Type, Value),
    Sextend(Type, Value),
    Ireduce(Type, Value),
    Load(Type, Value, i32),
    Store(Value, Value, i32),
    StackAddr(StackSlot, i32),
    Jump(Block, Vec<Value>),
    Brif(Value, Block, Vec<Value>, Block, Vec<Value>),
    Return_(Vec<Value>),
    Trap,
    Call(FuncRef, Vec<Value>, Vec<Type>),
    F64const(f64),
    F32const(f32),
    Fadd(Value, Value),
    Fsub(Value, Value),
    Fmul(Value, Value),
    Fdiv(Value, Value),
    Sqrt(Value),
    Floor(Value),
    Ceil(Value),
    Trunc(Value),
    Fneg(Value),
    Fabs(Value),
    Fcopysign(Value, Value),
    Fma(Value, Value, Value),
    Bitcast(Type, Value),
    Fcmp(FloatCC, Value, Value),
    Fpromote(Value),
    Fdemote(Value),
    FcvtFromSint(Type, Value),
    FcvtFromUint(Type, Value),
    FcvtToSintSat(Type, Value),
    FcvtToUintSat(Type, Value),
    Iconcat(Value, Value),
    Isplit(Value),
    Iadd128(Value, Value),
    Isub128(Value, Value),
    Band128(Value, Value),
    Bor128(Value, Value),
    Bxor128(Value, Value),
    Imul128(Value, Value),
    Udiv128(Value, Value),
    Sdiv128(Value, Value),
    Urem128(Value, Value),
    Srem128(Value, Value),
    Icmp128(IntCC, Value, Value),
    Ishl128(Value, Value),
    Ushr128(Value, Value),
    Sshr128(Value, Value),
    Extend128(Value, bool),
    Reduce128(Type, Value),
    UaddOverflow(Value, Value),
    UsubOverflow(Value, Value),
    SaddOverflow(Value, Value),
    SsubOverflow(Value, Value),
    UmulOverflow(Value, Value),
    SmulOverflow(Value, Value),
    Umulhi(Value, Value),
    Clz(Value),
    Ctz(Value),
    Popcnt(Value),
    Bswap(Value),
    Rotl(Value, Value),
    Rotr(Value, Value),
    Smin(Value, Value),
    Umin(Value, Value),
    Smax(Value, Value),
    Umax(Value, Value),
    Iabs(Value),
    TrapCode(u8),
    TrapzCode(Value, u8),
    TrapnzCode(Value, u8),
    Trapz(Value),
    Trapnz(Value),
    CallIndirect(Value, Vec<Value>, Vec<Type>),
    SymbolAddr(String),
}

/// 1 関数分の組み立て。命令は記録しながら解析用の Emitter で組み立て (値の番号と型を決める)、
/// finish で値・変数をレジスタに割り当ててから、本番の Emitter で記録した命令を同じ順にもう一度組み立てる
pub struct FunctionBuilder {
    params: Vec<Type>,
    rets: Vec<Type>,
    dry: Emitter,
    ops: Vec<Op>,
    real: Option<Emitter>,
}

impl FunctionBuilder {
    pub fn new(params: Vec<Type>, rets: Vec<Type>) -> FunctionBuilder {
        let dry = Emitter::new(params.clone(), rets.clone(), None);
        FunctionBuilder { params, rets, dry, ops: vec![], real: None }
    }
    pub fn func_params(&self) -> Vec<Value> {
        self.dry.func_params()
    }
    pub fn value_type(&self, v: Value) -> Type {
        self.dry.value_type(v)
    }
    pub fn block_params(&self, b: Block) -> Vec<Value> {
        self.dry.block_params(b)
    }
    pub fn current_block(&self) -> Option<Block> {
        self.dry.current_block()
    }
    /// 割り当てを決めて本番を組み立てる (1 回だけ)
    fn realize(&mut self) {
        if self.real.is_some() {
            return;
        }
        let mut tr = self.dry.take_track().unwrap_or_default();
        // (すぐ後が trap の呼び出し (panic) は戻ってこない: 呼び出しをまたぐとしない)
        let ops = &self.ops;
        tr.calls.retain(|&c| !matches!(ops.get(c as usize + 1), Some(Op::Trap | Op::TrapCode(_))));
        let pl = if std::env::var_os("MINCLIF_NO_REGALLOC").is_some() { Plan::default() } else { plan(&tr, self.dry.values.len(), self.dry.vars.len()) };
        let mut e = Emitter::new(self.params.clone(), self.rets.clone(), Some(pl));
        // (値ごとの読む回数: icmp の値を brif だけが読むなら、まとめて cmp + jcc にする)
        let mut reads = vec![0u32; self.dry.values.len()];
        for &(id, _, def) in &tr.ev {
            if !def && (id as usize) < reads.len() {
                reads[id as usize] += 1;
            }
        }
        let ops = std::mem::take(&mut self.ops);
        let mut i = 0;
        while i < ops.len() {
            e.op = i as u32;
            if let Op::Icmp(cc, a, b) = &ops[i] {
                // (間に値もコードも作らない命令 (ブロック・変数・スロットを作る) があってもよい)
                let mut j = i + 1;
                while j < ops.len() && matches!(ops[j], Op::CreateBlock | Op::DeclareVar(_) | Op::CreateStackSlot(..) | Op::ImportFunction(_)) {
                    j += 1;
                }
                let next = e.values.len() as u32;
                if let Some(Op::Brif(c, tb, ta, eb, ea)) = ops.get(j) {
                    if c.0 == next && reads.get(next as usize) == Some(&1) {
                        e.cmp_only(*cc, *a, *b);
                        for k in i + 1..j {
                            e.op = k as u32;
                            replay(&mut e, &ops[k]);
                        }
                        e.op = j as u32;
                        e.brif_cc(*cc, *tb, ta.as_slice(), *eb, ea.as_slice());
                        i = j + 1;
                        continue;
                    }
                }
            }
            replay(&mut e, &ops[i]);
            i += 1;
        }
        self.real = Some(e);
    }
    pub fn traps(&mut self) -> Vec<(u32, u8)> {
        self.realize();
        self.real.as_ref().unwrap().traps()
    }
    pub fn finish(mut self) -> (Vec<u8>, Vec<Reloc>) {
        self.realize();
        self.real.take().unwrap().finish()
    }
    pub fn finish_zeroing(mut self, zero: &[StackSlot]) -> (Vec<u8>, Vec<Reloc>) {
        self.realize();
        self.real.take().unwrap().finish_zeroing(zero)
    }
    pub fn create_block(&mut self) -> Block {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::CreateBlock);
        self.dry.create_block()
    }
    pub fn append_block_param(&mut self, b: Block, t: Type) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::AppendBlockParam(b, t));
        self.dry.append_block_param(b, t)
    }
    pub fn switch_to_block(&mut self, b: Block) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SwitchToBlock(b));
        self.dry.switch_to_block(b)
    }
    pub fn declare_var(&mut self, t: Type) -> Variable {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::DeclareVar(t));
        self.dry.declare_var(t)
    }
    pub fn def_var(&mut self, var: Variable, v: Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::DefVar(var, v));
        self.dry.def_var(var, v)
    }
    pub fn use_var(&mut self, var: Variable) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UseVar(var));
        self.dry.use_var(var)
    }
    pub fn create_stack_slot(&mut self, size: u32, align: u32) -> StackSlot {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::CreateStackSlot(size, align));
        self.dry.create_stack_slot(size, align)
    }
    pub fn import_function(&mut self, name: &str) -> FuncRef {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::ImportFunction(name.to_string()));
        self.dry.import_function(name)
    }
    pub fn iconst(&mut self, t: Type, x: i64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iconst(t, x));
        self.dry.iconst(t, x)
    }
    pub fn iadd(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iadd(a, b));
        self.dry.iadd(a, b)
    }
    pub fn isub(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Isub(a, b));
        self.dry.isub(a, b)
    }
    pub fn imul(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Imul(a, b));
        self.dry.imul(a, b)
    }
    pub fn band(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Band(a, b));
        self.dry.band(a, b)
    }
    pub fn bor(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bor(a, b));
        self.dry.bor(a, b)
    }
    pub fn bxor(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bxor(a, b));
        self.dry.bxor(a, b)
    }
    pub fn iadd_imm(&mut self, a: Value, x: i64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::IaddImm(a, x));
        self.dry.iadd_imm(a, x)
    }
    pub fn udiv(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Udiv(a, b));
        self.dry.udiv(a, b)
    }
    pub fn sdiv(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sdiv(a, b));
        self.dry.sdiv(a, b)
    }
    pub fn urem(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Urem(a, b));
        self.dry.urem(a, b)
    }
    pub fn srem(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Srem(a, b));
        self.dry.srem(a, b)
    }
    pub fn ishl(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ishl(a, b));
        self.dry.ishl(a, b)
    }
    pub fn ushr(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ushr(a, b));
        self.dry.ushr(a, b)
    }
    pub fn sshr(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sshr(a, b));
        self.dry.sshr(a, b)
    }
    pub fn ineg(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ineg(a));
        self.dry.ineg(a)
    }
    pub fn bnot(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bnot(a));
        self.dry.bnot(a)
    }
    pub fn icmp(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Icmp(cc, a, b));
        self.dry.icmp(cc, a, b)
    }
    pub fn icmp_imm(&mut self, cc: IntCC, a: Value, x: i64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::IcmpImm(cc, a, x));
        self.dry.icmp_imm(cc, a, x)
    }
    pub fn select(&mut self, c: Value, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Select(c, a, b));
        self.dry.select(c, a, b)
    }
    pub fn uextend(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Uextend(t, a));
        self.dry.uextend(t, a)
    }
    pub fn sextend(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sextend(t, a));
        self.dry.sextend(t, a)
    }
    pub fn ireduce(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ireduce(t, a));
        self.dry.ireduce(t, a)
    }
    pub fn load(&mut self, t: Type, addr: Value, off: i32) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Load(t, addr, off));
        self.dry.load(t, addr, off)
    }
    pub fn store(&mut self, v: Value, addr: Value, off: i32) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Store(v, addr, off));
        self.dry.store(v, addr, off)
    }
    pub fn stack_addr(&mut self, s: StackSlot, off: i32) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::StackAddr(s, off));
        self.dry.stack_addr(s, off)
    }
    pub fn jump(&mut self, b: Block, args: &[Value]) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Jump(b, args.to_vec()));
        self.dry.jump(b, args)
    }
    pub fn brif(&mut self, c: Value, then_b: Block, then_args: &[Value], else_b: Block, else_args: &[Value]) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Brif(c, then_b, then_args.to_vec(), else_b, else_args.to_vec()));
        self.dry.brif(c, then_b, then_args, else_b, else_args)
    }
    pub fn return_(&mut self, vals: &[Value]) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Return_(vals.to_vec()));
        self.dry.return_(vals)
    }
    pub fn trap(&mut self) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trap);
        self.dry.trap()
    }
    pub fn call(&mut self, f: FuncRef, args: &[Value], rets: &[Type]) -> Vec<Value> {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Call(f, args.to_vec(), rets.to_vec()));
        self.dry.call(f, args, rets)
    }
    pub fn f64const(&mut self, x: f64) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::F64const(x));
        self.dry.f64const(x)
    }
    pub fn f32const(&mut self, x: f32) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::F32const(x));
        self.dry.f32const(x)
    }
    pub fn fadd(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fadd(a, b));
        self.dry.fadd(a, b)
    }
    pub fn fsub(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fsub(a, b));
        self.dry.fsub(a, b)
    }
    pub fn fmul(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fmul(a, b));
        self.dry.fmul(a, b)
    }
    pub fn fdiv(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fdiv(a, b));
        self.dry.fdiv(a, b)
    }
    pub fn sqrt(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sqrt(a));
        self.dry.sqrt(a)
    }
    pub fn floor(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Floor(a));
        self.dry.floor(a)
    }
    pub fn ceil(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ceil(a));
        self.dry.ceil(a)
    }
    pub fn trunc(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trunc(a));
        self.dry.trunc(a)
    }
    pub fn fneg(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fneg(a));
        self.dry.fneg(a)
    }
    pub fn fabs(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fabs(a));
        self.dry.fabs(a)
    }
    pub fn fcopysign(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fcopysign(a, b));
        self.dry.fcopysign(a, b)
    }
    pub fn fma(&mut self, a: Value, b: Value, c: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fma(a, b, c));
        self.dry.fma(a, b, c)
    }
    pub fn bitcast(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bitcast(t, a));
        self.dry.bitcast(t, a)
    }
    pub fn fcmp(&mut self, cc: FloatCC, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fcmp(cc, a, b));
        self.dry.fcmp(cc, a, b)
    }
    pub fn fpromote(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fpromote(a));
        self.dry.fpromote(a)
    }
    pub fn fdemote(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Fdemote(a));
        self.dry.fdemote(a)
    }
    pub fn fcvt_from_sint(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtFromSint(t, a));
        self.dry.fcvt_from_sint(t, a)
    }
    pub fn fcvt_from_uint(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtFromUint(t, a));
        self.dry.fcvt_from_uint(t, a)
    }
    pub fn fcvt_to_sint_sat(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtToSintSat(t, a));
        self.dry.fcvt_to_sint_sat(t, a)
    }
    pub fn fcvt_to_uint_sat(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::FcvtToUintSat(t, a));
        self.dry.fcvt_to_uint_sat(t, a)
    }
    pub fn iconcat(&mut self, lo: Value, hi: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iconcat(lo, hi));
        self.dry.iconcat(lo, hi)
    }
    pub fn isplit(&mut self, v: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Isplit(v));
        self.dry.isplit(v)
    }
    pub fn iadd128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iadd128(a, b));
        self.dry.iadd128(a, b)
    }
    pub fn isub128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Isub128(a, b));
        self.dry.isub128(a, b)
    }
    pub fn band128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Band128(a, b));
        self.dry.band128(a, b)
    }
    pub fn bor128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bor128(a, b));
        self.dry.bor128(a, b)
    }
    pub fn bxor128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bxor128(a, b));
        self.dry.bxor128(a, b)
    }
    pub fn imul128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Imul128(a, b));
        self.dry.imul128(a, b)
    }
    pub fn udiv128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Udiv128(a, b));
        self.dry.udiv128(a, b)
    }
    pub fn sdiv128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sdiv128(a, b));
        self.dry.sdiv128(a, b)
    }
    pub fn urem128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Urem128(a, b));
        self.dry.urem128(a, b)
    }
    pub fn srem128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Srem128(a, b));
        self.dry.srem128(a, b)
    }
    pub fn icmp128(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Icmp128(cc, a, b));
        self.dry.icmp128(cc, a, b)
    }
    pub fn ishl128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ishl128(a, b));
        self.dry.ishl128(a, b)
    }
    pub fn ushr128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ushr128(a, b));
        self.dry.ushr128(a, b)
    }
    pub fn sshr128(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Sshr128(a, b));
        self.dry.sshr128(a, b)
    }
    pub fn extend128(&mut self, a: Value, signed: bool) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Extend128(a, signed));
        self.dry.extend128(a, signed)
    }
    pub fn reduce128(&mut self, t: Type, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Reduce128(t, a));
        self.dry.reduce128(t, a)
    }
    pub fn uadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UaddOverflow(a, b));
        self.dry.uadd_overflow(a, b)
    }
    pub fn usub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UsubOverflow(a, b));
        self.dry.usub_overflow(a, b)
    }
    pub fn sadd_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SaddOverflow(a, b));
        self.dry.sadd_overflow(a, b)
    }
    pub fn ssub_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SsubOverflow(a, b));
        self.dry.ssub_overflow(a, b)
    }
    pub fn umul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::UmulOverflow(a, b));
        self.dry.umul_overflow(a, b)
    }
    pub fn smul_overflow(&mut self, a: Value, b: Value) -> (Value, Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SmulOverflow(a, b));
        self.dry.smul_overflow(a, b)
    }
    pub fn umulhi(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Umulhi(a, b));
        self.dry.umulhi(a, b)
    }
    pub fn clz(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Clz(a));
        self.dry.clz(a)
    }
    pub fn ctz(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Ctz(a));
        self.dry.ctz(a)
    }
    pub fn popcnt(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Popcnt(a));
        self.dry.popcnt(a)
    }
    pub fn bswap(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Bswap(a));
        self.dry.bswap(a)
    }
    pub fn rotl(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Rotl(a, b));
        self.dry.rotl(a, b)
    }
    pub fn rotr(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Rotr(a, b));
        self.dry.rotr(a, b)
    }
    pub fn smin(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Smin(a, b));
        self.dry.smin(a, b)
    }
    pub fn umin(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Umin(a, b));
        self.dry.umin(a, b)
    }
    pub fn smax(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Smax(a, b));
        self.dry.smax(a, b)
    }
    pub fn umax(&mut self, a: Value, b: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Umax(a, b));
        self.dry.umax(a, b)
    }
    pub fn iabs(&mut self, a: Value) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Iabs(a));
        self.dry.iabs(a)
    }
    pub fn trap_code(&mut self, code: u8) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::TrapCode(code));
        self.dry.trap_code(code)
    }
    pub fn trapz_code(&mut self, c: Value, code: u8) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::TrapzCode(c, code));
        self.dry.trapz_code(c, code)
    }
    pub fn trapnz_code(&mut self, c: Value, code: u8) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::TrapnzCode(c, code));
        self.dry.trapnz_code(c, code)
    }
    pub fn trapz(&mut self, c: Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trapz(c));
        self.dry.trapz(c)
    }
    pub fn trapnz(&mut self, c: Value) {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::Trapnz(c));
        self.dry.trapnz(c)
    }
    pub fn call_indirect(&mut self, p: Value, args: &[Value], rets: &[Type]) -> Vec<Value> {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::CallIndirect(p, args.to_vec(), rets.to_vec()));
        self.dry.call_indirect(p, args, rets)
    }
    pub fn symbol_addr(&mut self, name: &str) -> Value {
        self.dry.op = self.ops.len() as u32;
        self.ops.push(Op::SymbolAddr(name.to_string()));
        self.dry.symbol_addr(name)
    }
}

/// 記録した命令を本番の Emitter で組み立てる (戻り値は解析のときと同じ番号になる)
fn replay(e: &mut Emitter, op: &Op) {
    match op {
        Op::CreateBlock => {
            let _ = e.create_block();
        }
        Op::AppendBlockParam(f0, f1) => {
            let _ = e.append_block_param(*f0, *f1);
        }
        Op::SwitchToBlock(f0) => {
            let _ = e.switch_to_block(*f0);
        }
        Op::DeclareVar(f0) => {
            let _ = e.declare_var(*f0);
        }
        Op::DefVar(f0, f1) => {
            let _ = e.def_var(*f0, *f1);
        }
        Op::UseVar(f0) => {
            let _ = e.use_var(*f0);
        }
        Op::CreateStackSlot(f0, f1) => {
            let _ = e.create_stack_slot(*f0, *f1);
        }
        Op::ImportFunction(f0) => {
            let _ = e.import_function(f0.as_str());
        }
        Op::Iconst(f0, f1) => {
            let _ = e.iconst(*f0, *f1);
        }
        Op::Iadd(f0, f1) => {
            let _ = e.iadd(*f0, *f1);
        }
        Op::Isub(f0, f1) => {
            let _ = e.isub(*f0, *f1);
        }
        Op::Imul(f0, f1) => {
            let _ = e.imul(*f0, *f1);
        }
        Op::Band(f0, f1) => {
            let _ = e.band(*f0, *f1);
        }
        Op::Bor(f0, f1) => {
            let _ = e.bor(*f0, *f1);
        }
        Op::Bxor(f0, f1) => {
            let _ = e.bxor(*f0, *f1);
        }
        Op::IaddImm(f0, f1) => {
            let _ = e.iadd_imm(*f0, *f1);
        }
        Op::Udiv(f0, f1) => {
            let _ = e.udiv(*f0, *f1);
        }
        Op::Sdiv(f0, f1) => {
            let _ = e.sdiv(*f0, *f1);
        }
        Op::Urem(f0, f1) => {
            let _ = e.urem(*f0, *f1);
        }
        Op::Srem(f0, f1) => {
            let _ = e.srem(*f0, *f1);
        }
        Op::Ishl(f0, f1) => {
            let _ = e.ishl(*f0, *f1);
        }
        Op::Ushr(f0, f1) => {
            let _ = e.ushr(*f0, *f1);
        }
        Op::Sshr(f0, f1) => {
            let _ = e.sshr(*f0, *f1);
        }
        Op::Ineg(f0) => {
            let _ = e.ineg(*f0);
        }
        Op::Bnot(f0) => {
            let _ = e.bnot(*f0);
        }
        Op::Icmp(f0, f1, f2) => {
            let _ = e.icmp(*f0, *f1, *f2);
        }
        Op::IcmpImm(f0, f1, f2) => {
            let _ = e.icmp_imm(*f0, *f1, *f2);
        }
        Op::Select(f0, f1, f2) => {
            let _ = e.select(*f0, *f1, *f2);
        }
        Op::Uextend(f0, f1) => {
            let _ = e.uextend(*f0, *f1);
        }
        Op::Sextend(f0, f1) => {
            let _ = e.sextend(*f0, *f1);
        }
        Op::Ireduce(f0, f1) => {
            let _ = e.ireduce(*f0, *f1);
        }
        Op::Load(f0, f1, f2) => {
            let _ = e.load(*f0, *f1, *f2);
        }
        Op::Store(f0, f1, f2) => {
            let _ = e.store(*f0, *f1, *f2);
        }
        Op::StackAddr(f0, f1) => {
            let _ = e.stack_addr(*f0, *f1);
        }
        Op::Jump(f0, f1) => {
            let _ = e.jump(*f0, f1.as_slice());
        }
        Op::Brif(f0, f1, f2, f3, f4) => {
            let _ = e.brif(*f0, *f1, f2.as_slice(), *f3, f4.as_slice());
        }
        Op::Return_(f0) => {
            let _ = e.return_(f0.as_slice());
        }
        Op::Trap => {
            let _ = e.trap();
        }
        Op::Call(f0, f1, f2) => {
            let _ = e.call(*f0, f1.as_slice(), f2.as_slice());
        }
        Op::F64const(f0) => {
            let _ = e.f64const(*f0);
        }
        Op::F32const(f0) => {
            let _ = e.f32const(*f0);
        }
        Op::Fadd(f0, f1) => {
            let _ = e.fadd(*f0, *f1);
        }
        Op::Fsub(f0, f1) => {
            let _ = e.fsub(*f0, *f1);
        }
        Op::Fmul(f0, f1) => {
            let _ = e.fmul(*f0, *f1);
        }
        Op::Fdiv(f0, f1) => {
            let _ = e.fdiv(*f0, *f1);
        }
        Op::Sqrt(f0) => {
            let _ = e.sqrt(*f0);
        }
        Op::Floor(f0) => {
            let _ = e.floor(*f0);
        }
        Op::Ceil(f0) => {
            let _ = e.ceil(*f0);
        }
        Op::Trunc(f0) => {
            let _ = e.trunc(*f0);
        }
        Op::Fneg(f0) => {
            let _ = e.fneg(*f0);
        }
        Op::Fabs(f0) => {
            let _ = e.fabs(*f0);
        }
        Op::Fcopysign(f0, f1) => {
            let _ = e.fcopysign(*f0, *f1);
        }
        Op::Fma(f0, f1, f2) => {
            let _ = e.fma(*f0, *f1, *f2);
        }
        Op::Bitcast(f0, f1) => {
            let _ = e.bitcast(*f0, *f1);
        }
        Op::Fcmp(f0, f1, f2) => {
            let _ = e.fcmp(*f0, *f1, *f2);
        }
        Op::Fpromote(f0) => {
            let _ = e.fpromote(*f0);
        }
        Op::Fdemote(f0) => {
            let _ = e.fdemote(*f0);
        }
        Op::FcvtFromSint(f0, f1) => {
            let _ = e.fcvt_from_sint(*f0, *f1);
        }
        Op::FcvtFromUint(f0, f1) => {
            let _ = e.fcvt_from_uint(*f0, *f1);
        }
        Op::FcvtToSintSat(f0, f1) => {
            let _ = e.fcvt_to_sint_sat(*f0, *f1);
        }
        Op::FcvtToUintSat(f0, f1) => {
            let _ = e.fcvt_to_uint_sat(*f0, *f1);
        }
        Op::Iconcat(f0, f1) => {
            let _ = e.iconcat(*f0, *f1);
        }
        Op::Isplit(f0) => {
            let _ = e.isplit(*f0);
        }
        Op::Iadd128(f0, f1) => {
            let _ = e.iadd128(*f0, *f1);
        }
        Op::Isub128(f0, f1) => {
            let _ = e.isub128(*f0, *f1);
        }
        Op::Band128(f0, f1) => {
            let _ = e.band128(*f0, *f1);
        }
        Op::Bor128(f0, f1) => {
            let _ = e.bor128(*f0, *f1);
        }
        Op::Bxor128(f0, f1) => {
            let _ = e.bxor128(*f0, *f1);
        }
        Op::Imul128(f0, f1) => {
            let _ = e.imul128(*f0, *f1);
        }
        Op::Udiv128(f0, f1) => {
            let _ = e.udiv128(*f0, *f1);
        }
        Op::Sdiv128(f0, f1) => {
            let _ = e.sdiv128(*f0, *f1);
        }
        Op::Urem128(f0, f1) => {
            let _ = e.urem128(*f0, *f1);
        }
        Op::Srem128(f0, f1) => {
            let _ = e.srem128(*f0, *f1);
        }
        Op::Icmp128(f0, f1, f2) => {
            let _ = e.icmp128(*f0, *f1, *f2);
        }
        Op::Ishl128(f0, f1) => {
            let _ = e.ishl128(*f0, *f1);
        }
        Op::Ushr128(f0, f1) => {
            let _ = e.ushr128(*f0, *f1);
        }
        Op::Sshr128(f0, f1) => {
            let _ = e.sshr128(*f0, *f1);
        }
        Op::Extend128(f0, f1) => {
            let _ = e.extend128(*f0, *f1);
        }
        Op::Reduce128(f0, f1) => {
            let _ = e.reduce128(*f0, *f1);
        }
        Op::UaddOverflow(f0, f1) => {
            let _ = e.uadd_overflow(*f0, *f1);
        }
        Op::UsubOverflow(f0, f1) => {
            let _ = e.usub_overflow(*f0, *f1);
        }
        Op::SaddOverflow(f0, f1) => {
            let _ = e.sadd_overflow(*f0, *f1);
        }
        Op::SsubOverflow(f0, f1) => {
            let _ = e.ssub_overflow(*f0, *f1);
        }
        Op::UmulOverflow(f0, f1) => {
            let _ = e.umul_overflow(*f0, *f1);
        }
        Op::SmulOverflow(f0, f1) => {
            let _ = e.smul_overflow(*f0, *f1);
        }
        Op::Umulhi(f0, f1) => {
            let _ = e.umulhi(*f0, *f1);
        }
        Op::Clz(f0) => {
            let _ = e.clz(*f0);
        }
        Op::Ctz(f0) => {
            let _ = e.ctz(*f0);
        }
        Op::Popcnt(f0) => {
            let _ = e.popcnt(*f0);
        }
        Op::Bswap(f0) => {
            let _ = e.bswap(*f0);
        }
        Op::Rotl(f0, f1) => {
            let _ = e.rotl(*f0, *f1);
        }
        Op::Rotr(f0, f1) => {
            let _ = e.rotr(*f0, *f1);
        }
        Op::Smin(f0, f1) => {
            let _ = e.smin(*f0, *f1);
        }
        Op::Umin(f0, f1) => {
            let _ = e.umin(*f0, *f1);
        }
        Op::Smax(f0, f1) => {
            let _ = e.smax(*f0, *f1);
        }
        Op::Umax(f0, f1) => {
            let _ = e.umax(*f0, *f1);
        }
        Op::Iabs(f0) => {
            let _ = e.iabs(*f0);
        }
        Op::TrapCode(f0) => {
            let _ = e.trap_code(*f0);
        }
        Op::TrapzCode(f0, f1) => {
            let _ = e.trapz_code(*f0, *f1);
        }
        Op::TrapnzCode(f0, f1) => {
            let _ = e.trapnz_code(*f0, *f1);
        }
        Op::Trapz(f0) => {
            let _ = e.trapz(*f0);
        }
        Op::Trapnz(f0) => {
            let _ = e.trapnz(*f0);
        }
        Op::CallIndirect(f0, f1, f2) => {
            let _ = e.call_indirect(*f0, f1.as_slice(), f2.as_slice());
        }
        Op::SymbolAddr(f0) => {
            let _ = e.symbol_addr(f0.as_str());
        }
    }
}
