"""crustc 自身の crates/minclif/src/lib.rs (Cranelift の代わりの最小のバックエンド) を 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_minclif.py > out/selfhost/minclif_main.rs
main は tests/minclif.rs と同じ関数 (ループ・再帰・変数・スタックスロット・8 引数、浮動小数点・i128・オーバーフロー・
ビット演算・間接呼び出し・データ) を組み立て、引数の 2 つのパスに ELF の .o を書き、2 つ目の期待する出力を出す。"""
src = open("crates/minclif/src/lib.rs").read()
test = open("tests/minclif.rs").read()
a = test.index("/// sum(n)")
b = test.index("/// main: printf")
fns = test[a:b].replace("crustc::minclif::", "minclif::")
# 浮動小数点・i128・オーバーフロー・ビット演算・間接呼び出し・データの関数
a2 = test.index("/// 浮動小数点・i128・オーバーフロー")
b2 = test.index("#[test]\nfn ext_ops()")
fns += test[a2:b2].replace("crustc::minclif::", "minclif::")
print("// 自動生成: tests/selfhost/build_minclif.py (crustc の crates/minclif/src/lib.rs をそのまま含む)\nmod minclif {\n" + src + "}\n")
print("use minclif::{FunctionBuilder, IntCC, Type, write_object};\n")
print(fns)
print('''fn main() {
    let args: Vec<String> = std::env::args().collect();
    let funcs = vec![sum(), fib(), misc()];
    let obj = write_object(&funcs);
    std::fs::write(&args[1], &obj).unwrap();
    // 2 つ目: ext の関数 (+ sum) とデータ。期待する出力を標準出力に
    let (mut f2, datas, want) = ext();
    f2.push(sum());
    let obj2 = minclif::write_object_full(&f2, &datas);
    std::fs::write(&args[2], &obj2).unwrap();
    for w in want {
        println!("{w}");
    }
}
''')
