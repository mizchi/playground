"""N 個の関数を持つ (rustc でもコンパイルできる) ベンチ用ソースを生成する。
usage: python3 bench/gen.py N [variant] > out.rs
variant を変えると f{N//2} の本体だけが変わる (1 関数編集のシミュレーション)。"""
import sys

n = int(sys.argv[1])
variant = int(sys.argv[2]) if len(sys.argv) > 2 else 0
print('unsafe extern "C" {\n    fn putchar(c: i32) -> i32;\n}\n')
print("fn print_u64(n: u64) {\n    if n >= 10 {\n        print_u64(n / 10);\n    }\n"
      "    unsafe {\n        putchar((b'0' + (n % 10) as u8) as i32);\n    }\n}\n")
for i in range(n):
    k = (i * 7 + 3) % 13 + 1
    extra = variant if i == n // 2 else 0
    callee = f"f{i - 1}(x / 2 + {k})" if i > 0 else "x"
    # 実行時間が指数的に増えないよう、2 本目の呼び出しは浅いところだけ
    other = f" + f{i // 2}(x % {k + 1})" if 1 < i < 300 and i % 3 == 0 else ""
    print(f"""fn f{i}(x: u64) -> u64 {{
    let mut acc: u64 = {k + extra};
    let mut j = 0;
    while j < (x % 4) {{
        if (acc ^ j) % 3 == 0 {{
            acc = acc * {k} + j;
        }} else {{
            acc += j << 1;
        }}
        j += 1;
    }}
    if x == 0 {{
        return acc;
    }}
    acc % 1000 + {callee}{other} % 7
}}
""")
print(f"fn main() {{\n    print_u64(f{n - 1}(12345));\n    unsafe {{\n        putchar(10);\n    }}\n}}")
