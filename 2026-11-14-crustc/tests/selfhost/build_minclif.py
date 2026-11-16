"""crustc 自身の src/minclif.rs (Cranelift の代わりの最小のバックエンド) を 1 ファイルのプログラムにまとめる。
usage: python3 tests/selfhost/build_minclif.py > out/selfhost/minclif_main.rs
main は tests/minclif.rs と同じ関数 (ループ・再帰・変数・スタックスロット・8 引数) を組み立て、引数のパスに ELF の .o を書く。"""
src = open("src/minclif.rs").read()
test = open("tests/minclif.rs").read()
a = test.index("/// sum(n)")
b = test.index("/// main: printf")
fns = test[a:b].replace("crustc::minclif::", "minclif::")
print("// 自動生成: tests/selfhost/build_minclif.py (crustc の src/minclif.rs をそのまま含む)\nmod minclif {\n" + src + "}\n")
print("use minclif::{FunctionBuilder, IntCC, Type, write_object};\n")
print(fns)
print('''fn main() {
    let args: Vec<String> = std::env::args().collect();
    let funcs = vec![sum(), fib(), misc()];
    let obj = write_object(&funcs);
    std::fs::write(&args[1], &obj).unwrap();
    println!("{} bytes", obj.len());
}
''')
