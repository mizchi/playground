//! ベンチ・テスト用のプログラム生成器 (`bench/gen.py` と同じ出力)。
//! `variant` を変えると `f{n/2}` の本体だけが変わる (1 関数編集のシミュレーション)。

use std::fmt::Write;

pub fn program(n: usize, variant: usize) -> String {
    let mut s = String::new();
    s.push_str("unsafe extern \"C\" {\n    fn putchar(c: i32) -> i32;\n}\n\n");
    s.push_str("fn print_u64(n: u64) {\n    if n >= 10 {\n        print_u64(n / 10);\n    }\n    unsafe {\n        putchar((b'0' + (n % 10) as u8) as i32);\n    }\n}\n\n");
    for i in 0..n {
        let k = (i * 7 + 3) % 13 + 1;
        let extra = if i == n / 2 { variant } else { 0 };
        let callee = if i > 0 { format!("f{}(x / 2 + {k})", i - 1) } else { "x".into() };
        let other = if 1 < i && i < 300 && i % 3 == 0 { format!(" + f{}(x % {})", i / 2, k + 1) } else { String::new() };
        writeln!(
            s,
            "fn f{i}(x: u64) -> u64 {{
    let mut acc: u64 = {a};
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
",
            a = k + extra
        )
        .unwrap();
    }
    writeln!(s, "fn main() {{\n    print_u64(f{}(12345));\n    unsafe {{\n        putchar(10);\n    }}\n}}", n - 1).unwrap();
    s
}
