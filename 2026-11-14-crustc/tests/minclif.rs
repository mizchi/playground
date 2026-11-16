//! 最小のバックエンド (crates/minclif/src/lib.rs) で関数を組み立て、ELF の .o にして cc でリンクし、動かして出力を確かめる。
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

/// 浮動小数点・i128・オーバーフロー・ビット演算・間接呼び出し・データ: 1 関数の中で計算して printf で出す
fn ext() -> (Vec<(String, Vec<u8>, Vec<crustc::minclif::Reloc>, bool)>, Vec<crustc::minclif::DataDef>, Vec<String>) {
    use crustc::minclif::FloatCC;
    let mut b = FunctionBuilder::new(vec![], vec![Type::I64]);
    let printf = b.import_function("printf");
    let mut want: Vec<String> = vec![];
    // 出す: 整数は %ld、ビット列は %lx
    macro_rules! pr_d {
        ($v:expr, $w:expr) => {{
            let f = b.symbol_addr("mc_fmt_d");
            let v = $v;
            let v = match b.value_type(v) {
                Type::I64 => v,
                _ => b.sextend(Type::I64, v),
            };
            b.call(printf, &[f, v], &[Type::I32]);
            want.push(format!("{}", $w));
        }};
    }
    macro_rules! pr_x {
        ($v:expr, $w:expr) => {{
            let f = b.symbol_addr("mc_fmt_x");
            let v = $v;
            let v = b.bitcast(Type::I64, v);
            b.call(printf, &[f, v], &[Type::I32]);
            want.push(format!("{:x}", $w));
        }};
    }
    // 浮動小数点
    let x = b.f64const(1.5);
    let y = b.f64const(2.25);
    let four = b.f64const(4.0);
    let three = b.f64const(3.0);
    let s1 = b.fadd(x, y);
    let s2 = b.fmul(s1, four);
    let s3 = b.fdiv(s2, three);
    pr_x!(s3, ((1.5f64 + 2.25) * 4.0 / 3.0).to_bits());
    let two = b.f64const(2.0);
    let r = b.sqrt(two);
    pr_x!(r, 2f64.sqrt().to_bits());
    let m = b.f64const(-1.7);
    let f1 = b.floor(m);
    pr_x!(f1, (-1.7f64).floor().to_bits());
    let c1 = b.ceil(m);
    pr_x!(c1, (-1.7f64).ceil().to_bits());
    let t1 = b.trunc(m);
    pr_x!(t1, (-1.7f64).trunc().to_bits());
    let n1 = b.fneg(m);
    pr_x!(n1, (1.7f64).to_bits());
    let a1 = b.fabs(m);
    pr_x!(a1, (1.7f64).to_bits());
    let cs = b.fcopysign(two, m);
    pr_x!(cs, (-2.0f64).to_bits());
    let nan = b.f64const(f64::NAN);
    let lt = b.fcmp(FloatCC::LessThan, x, y);
    pr_d!(lt, 1);
    let ge = b.fcmp(FloatCC::GreaterThanOrEqual, x, y);
    pr_d!(ge, 0);
    let eqn = b.fcmp(FloatCC::Equal, nan, nan);
    pr_d!(eqn, 0);
    let nen = b.fcmp(FloatCC::NotEqual, nan, nan);
    pr_d!(nen, 1);
    let le = b.fcmp(FloatCC::LessThanOrEqual, nan, x);
    pr_d!(le, 0);
    // 変換
    let k = b.iconst(Type::I32, -7);
    let kf = b.fcvt_from_sint(Type::F64, k);
    pr_x!(kf, (-7.0f64).to_bits());
    let umax = b.iconst(Type::I64, -1);
    let uf = b.fcvt_from_uint(Type::F64, umax);
    pr_x!(uf, (u64::MAX as f64).to_bits());
    let big = b.f64const(3e10);
    let c32 = b.fcvt_to_sint_sat(Type::I32, big);
    pr_d!(c32, 3e10f64 as i32);
    let cn = b.fcvt_to_sint_sat(Type::I32, nan);
    pr_d!(cn, f64::NAN as i32);
    let neg = b.f64const(-1e30);
    let c64 = b.fcvt_to_sint_sat(Type::I64, neg);
    pr_d!(c64, -1e30f64 as i64);
    let f300 = b.f64const(300.0);
    let cu8 = b.fcvt_to_uint_sat(Type::I8, f300);
    let cu8 = b.uextend(Type::I64, cu8);
    pr_d!(cu8, 300.0f64 as u8);
    let f15 = b.f64const(1.5e19);
    let cu64 = b.fcvt_to_uint_sat(Type::I64, f15);
    pr_x!(cu64, 1.5e19f64 as u64);
    let fm3 = b.f64const(-3.0);
    let cu0 = b.fcvt_to_uint_sat(Type::I64, fm3);
    pr_d!(cu0, -3.0f64 as u64);
    let p11 = b.f64const(1.1);
    let d = b.fdemote(p11);
    let pp = b.fpromote(d);
    pr_x!(pp, ((1.1f64 as f32) as f64).to_bits());
    let ff = b.f32const(0.5);
    let ff2 = b.fadd(ff, ff);
    let ffx = b.bitcast(Type::I32, ff2);
    let ffx = b.uextend(Type::I64, ffx);
    pr_x!(ffx, 1.0f32.to_bits() as u64);
    // i128
    let lo = b.iconst(Type::I64, 5);
    let hi = b.iconst(Type::I64, 1);
    let w = b.iconcat(lo, hi); // 2^64 + 5
    let w3lo = b.iconst(Type::I64, 3);
    let z = b.iconst(Type::I64, 0);
    let w3 = b.iconcat(w3lo, z);
    let wm = b.imul128(w, w3);
    let w7lo = b.iconst(Type::I64, 7);
    let w7 = b.iconcat(w7lo, z);
    let wd = b.udiv128(wm, w7);
    let wr = b.urem128(wm, w7);
    let wa = b.iadd128(wd, wr);
    let (alo, ahi) = b.isplit(wa);
    let want128 = ((1u128 << 64) + 5) * 3 / 7 + ((1u128 << 64) + 5) * 3 % 7;
    pr_x!(alo, want128 as u64);
    pr_x!(ahi, (want128 >> 64) as u64);
    let s70 = b.iconst(Type::I64, 70);
    let sh = b.ishl128(w, s70);
    let (_, shi) = b.isplit(sh);
    pr_x!(shi, ((((1u128 << 64) + 5) << 70) >> 64) as u64);
    let mlo = b.iconst(Type::I64, -1);
    let mn = b.iconcat(mlo, mlo); // -1
    let sr = b.sshr128(mn, s70);
    let (srlo, _) = b.isplit(sr);
    pr_x!(srlo, ((-1i128 >> 70) as u128) as u64);
    let ur = b.ushr128(mn, s70);
    let (urlo, _) = b.isplit(ur);
    pr_x!(urlo, (u128::MAX >> 70) as u64);
    let slt = b.icmp128(IntCC::SignedLessThan, mn, w);
    pr_d!(slt, 1);
    let ult = b.icmp128(IntCC::UnsignedLessThan, mn, w);
    pr_d!(ult, 0);
    let ge128 = b.icmp128(IntCC::UnsignedGreaterThanOrEqual, w, w);
    pr_d!(ge128, 1);
    let eq128 = b.icmp128(IntCC::Equal, w, w3);
    pr_d!(eq128, 0);
    let nine = b.iconst(Type::I32, -9);
    let e128 = b.extend128(nine, true);
    let (_, ehi) = b.isplit(e128);
    pr_x!(ehi, u64::MAX);
    let sq = b.sdiv128(e128, w3);
    let sqr = b.reduce128(Type::I64, sq);
    pr_d!(sqr, -3);
    let rem = b.srem128(e128, w7);
    let remr = b.reduce128(Type::I64, rem);
    pr_d!(remr, -9i128 % 7);
    // オーバーフロー
    let a200 = b.iconst(Type::I8, 200);
    let a100 = b.iconst(Type::I8, 100);
    let (v, of) = b.uadd_overflow(a200, a100);
    let v = b.uextend(Type::I64, v);
    pr_d!(v, 200u8.wrapping_add(100));
    pr_d!(of, 1);
    let imax = b.iconst(Type::I32, i32::MAX as i64);
    let one = b.iconst(Type::I32, 1);
    let (v, of) = b.sadd_overflow(imax, one);
    pr_d!(v, i32::MIN);
    pr_d!(of, 1);
    let a3 = b.iconst(Type::I16, 3);
    let a5 = b.iconst(Type::I16, 5);
    let (v, of) = b.usub_overflow(a3, a5);
    let v = b.uextend(Type::I64, v);
    pr_d!(v, 3u16.wrapping_sub(5));
    pr_d!(of, 1);
    let (v, of) = b.ssub_overflow(a3, a5);
    pr_d!(v, -2);
    pr_d!(of, 0);
    let p40 = b.iconst(Type::I64, 1 << 40);
    let p30 = b.iconst(Type::I64, 1 << 30);
    let (_, of) = b.umul_overflow(p40, p30);
    pr_d!(of, 1);
    let p62 = b.iconst(Type::I64, 1 << 62);
    let m2 = b.iconst(Type::I64, -2);
    let (v, of) = b.smul_overflow(p62, m2);
    pr_d!(v, (1i64 << 62).wrapping_mul(-2));
    pr_d!(of, 0);
    let two64 = b.iconst(Type::I64, 2);
    let (_, of) = b.smul_overflow(p62, two64);
    pr_d!(of, 1);
    let b16 = b.iconst(Type::I32, 70000);
    let (v, of) = b.umul_overflow(b16, b16);
    let v = b.uextend(Type::I64, v);
    pr_d!(v, 70000u32.wrapping_mul(70000));
    pr_d!(of, 1);
    let (_, of) = b.smul_overflow(a3, a5);
    pr_d!(of, 0);
    // ビット演算
    let i1 = b.iconst(Type::I32, 1);
    let cz = b.clz(i1);
    pr_d!(cz, 31);
    let z32 = b.iconst(Type::I32, 0);
    let cz0 = b.clz(z32);
    pr_d!(cz0, 32);
    let i8_ = b.iconst(Type::I64, 8);
    let tz = b.ctz(i8_);
    pr_d!(tz, 3);
    let tz0 = b.ctz(z32);
    pr_d!(tz0, 32);
    let pc = b.iconst(Type::I32, 0xFF0F);
    let pcn = b.popcnt(pc);
    pr_d!(pcn, 12);
    let bs = b.iconst(Type::I32, 0x11223344);
    let bsw = b.bswap(bs);
    let bsw = b.uextend(Type::I64, bsw);
    pr_x!(bsw, 0x44332211u64);
    let r81 = b.iconst(Type::I8, 0x81);
    let one8 = b.iconst(Type::I8, 1);
    let rl = b.rotl(r81, one8);
    let rl = b.uextend(Type::I64, rl);
    pr_d!(rl, 0x81u8.rotate_left(1));
    let rr = b.rotr(r81, one8);
    let rr = b.uextend(Type::I64, rr);
    pr_d!(rr, 0x81u8.rotate_right(1));
    let mn5 = b.iconst(Type::I64, -5);
    let p4 = b.iconst(Type::I64, 4);
    let um = b.umin(mn5, p4);
    pr_d!(um, 4);
    let sm = b.smin(mn5, p4);
    pr_d!(sm, -5);
    let sx = b.smax(mn5, p4);
    pr_d!(sx, 4);
    let ux = b.umax(mn5, p4);
    pr_d!(ux, -5);
    let ab = b.iabs(nine);
    pr_d!(ab, 9);
    let three64 = b.iconst(Type::I64, 3);
    let mh = b.umulhi(umax, three64);
    pr_d!(mh, 2);
    let sh8 = b.iconst(Type::I64, 3);
    let ushr = b.ushr(mn5, sh8);
    pr_d!(ushr, ((-5i64) as u64 >> 3) as i64);
    let sshr = b.sshr(mn5, sh8);
    pr_d!(sshr, -5i64 >> 3);
    // 間接呼び出し・データ (表の中のポインタ)
    let fp = b.symbol_addr("mc_sum");
    let ten = b.iconst(Type::I64, 10);
    let r = b.call_indirect(fp, &[ten], &[Type::I64])[0];
    pr_d!(r, 45);
    let tbl = b.symbol_addr("mc_tbl");
    let sptr = b.load(Type::I64, tbl, 8);
    let fs = b.symbol_addr("mc_fmt_s");
    b.call(printf, &[fs, sptr], &[Type::I32]);
    want.push("hello".into());
    let tv = b.load(Type::I64, tbl, 0);
    pr_d!(tv, 1234);
    let zr = b.iconst(Type::I64, 0);
    b.return_(&[zr]);
    let (c, r) = b.finish();
    let mut tbl_bytes = 1234i64.to_le_bytes().to_vec();
    tbl_bytes.extend_from_slice(&[0; 8]);
    let datas = vec![
        ("mc_fmt_d".to_string(), b"%ld\n\0".to_vec(), vec![], false),
        ("mc_fmt_x".to_string(), b"%lx\n\0".to_vec(), vec![], false),
        ("mc_fmt_s".to_string(), b"%s\n\0".to_vec(), vec![], false),
        ("mc_hello".to_string(), b"hello\0".to_vec(), vec![], false),
        ("mc_tbl".to_string(), tbl_bytes, vec![(8, "mc_hello".to_string(), 0)], true),
    ];
    (vec![("mc_ext".into(), c, r, true)], datas, want)
}

#[test]
fn ext_ops() {
    let (mut funcs, datas, want) = ext();
    funcs.push(sum());
    let obj = crustc::minclif::write_object_full(&funcs, &datas);
    let dir = std::env::temp_dir().join(format!("minclif_ext_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("f.o"), &obj).unwrap();
    std::fs::write(dir.join("main.c"), "long mc_ext(void);\nint main(){return (int)mc_ext();}\n").unwrap();
    let st = std::process::Command::new("cc").arg(dir.join("main.c")).arg(dir.join("f.o")).arg("-o").arg(dir.join("a.out")).status().unwrap();
    assert!(st.success(), "link failed");
    let out = std::process::Command::new(dir.join("a.out")).output().unwrap();
    assert!(out.status.success(), "crashed: {:?}", out.status);
    let got: Vec<String> = String::from_utf8_lossy(&out.stdout).lines().map(|l| l.to_string()).collect();
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "line {i}");
    }
    assert_eq!(got.len(), want.len());
}
