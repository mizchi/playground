//! 最小のバックエンド (src/minclif.rs) で関数を組み立て、ELF の .o にして cc でリンクし、動かして出力を確かめる。
use crustc::minclif::{FunctionBuilder, IntCC, Type, write_object};

/// sum(n) = 0 + 1 + .. + (n - 1) (ブロック引数のループ)
fn sum() -> (String, Vec<u8>, Vec<crustc::minclif::Reloc>, bool) {
    let mut b = FunctionBuilder::new(vec![Type::I64], vec![Type::I64]);
    let n = b.func_params()[0];
    let head = b.create_block();
    let i = b.append_block_param(head, Type::I64);
    let acc = b.append_block_param(head, Type::I64);
    let body = b.create_block();
    let exit = b.create_block();
    let z = b.iconst(Type::I64, 0);
    b.jump(head, &[z, z]);
    b.switch_to_block(head);
    let c = b.icmp(IntCC::SignedLessThan, i, n);
    b.brif(c, body, &[], exit, &[]);
    b.switch_to_block(body);
    let acc2 = b.iadd(acc, i);
    let i2 = b.iadd_imm(i, 1);
    b.jump(head, &[i2, acc2]);
    b.switch_to_block(exit);
    b.return_(&[acc]);
    let (c, r) = b.finish();
    ("mc_sum".into(), c, r, true)
}

/// fib(n) (再帰)
fn fib() -> (String, Vec<u8>, Vec<crustc::minclif::Reloc>, bool) {
    let mut b = FunctionBuilder::new(vec![Type::I32], vec![Type::I32]);
    let n = b.func_params()[0];
    let me = b.import_function("mc_fib");
    let small = b.create_block();
    let big = b.create_block();
    let c = b.icmp_imm(IntCC::SignedLessThan, n, 2);
    b.brif(c, small, &[], big, &[]);
    b.switch_to_block(small);
    b.return_(&[n]);
    b.switch_to_block(big);
    let a = b.iadd_imm(n, -1);
    let x = b.call(me, &[a], &[Type::I32])[0];
    let a2 = b.iadd_imm(n, -2);
    let y = b.call(me, &[a2], &[Type::I32])[0];
    let r = b.iadd(x, y);
    b.return_(&[r]);
    let (c, r) = b.finish();
    ("mc_fib".into(), c, r, true)
}

/// 変数・スタックスロット・読み書き・割り算・符号つきの比較・select・8 個の引数
fn misc() -> (String, Vec<u8>, Vec<crustc::minclif::Reloc>, bool) {
    let ps = vec![Type::I64; 8];
    let mut b = FunctionBuilder::new(ps, vec![Type::I64]);
    let a = b.func_params();
    let v = b.declare_var(Type::I64);
    let z = b.iconst(Type::I64, 0);
    b.def_var(v, z);
    for &x in &a {
        let cur = b.use_var(v);
        let s = b.iadd(cur, x);
        b.def_var(v, s);
    }
    // スロットに i32 と i8 を書いて読む
    let ss = b.create_stack_slot(16, 8);
    let p = b.stack_addr(ss, 0);
    let k = b.iconst(Type::I32, -5);
    b.store(k, p, 0);
    let k8 = b.iconst(Type::I8, 200);
    b.store(k8, p, 4);
    let r32 = b.load(Type::I32, p, 0);
    let r64 = b.sextend(Type::I64, r32); // -5
    let r8 = b.load(Type::I8, p, 4);
    let u = b.uextend(Type::I64, r8); // 200
    let neg = b.icmp_imm(IntCC::SignedLessThan, r32, 0); // 1
    let sel = b.select(neg, u, r64); // 200
    let total = b.use_var(v);
    let seven = b.iconst(Type::I64, 7);
    let q = b.sdiv(r64, seven); // -5 / 7 = 0
    let rem = b.srem(total, seven);
    let t1 = b.iadd(sel, rem);
    let t2 = b.iadd(t1, q);
    let t3 = b.imul(t2, r64); // * -5
    b.return_(&[t3]);
    let (c, r) = b.finish();
    ("mc_misc".into(), c, r, true)
}

/// main: printf で結果を出す (外部関数の呼び出し・文字列は呼び出し側の C から渡す)
#[test]
fn link_and_run() {
    let funcs = vec![sum(), fib(), misc()];
    let obj = write_object(&funcs);
    let dir = std::env::temp_dir().join(format!("minclif_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.o"), &obj).unwrap();
    std::fs::write(
        dir.join("main.c"),
        "#include <stdio.h>\nlong mc_sum(long);\nint mc_fib(int);\nlong mc_misc(long,long,long,long,long,long,long,long);\n\
         int main(){printf(\"%ld %d %ld\\n\", mc_sum(10), mc_fib(20), mc_misc(1,2,3,4,5,6,7,8));return 0;}\n",
    )
    .unwrap();
    let st = std::process::Command::new("cc")
        .arg(dir.join("main.c"))
        .arg(dir.join("f.o"))
        .arg("-o")
        .arg(dir.join("a.out"))
        .status()
        .unwrap();
    assert!(st.success(), "link failed");
    let out = std::process::Command::new(dir.join("a.out")).output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    // misc: 合計 36、36 % 7 = 1、(200 + 1 + 0) * -5 = -1005
    assert_eq!(s.trim(), "45 6765 -1005");
}
