# cratebench

crates.io の小さなクレートを crustc でビルドできるか・どのくらい速いかを rustc と比べる。

- `vendor/<名前>/`: クレートのソース (crates.io の配布物の `src/` と LICENSE をそのまま)
- `apps/<名前>/main.rs`: そのクレートを使うプログラム (最後に `done` を出す。edit の計測はこの行を書き換える)
- `crates.txt`: 名前・edition・feature
- `run.py`: `python3 cratebench/run.py [名前..]` (experiment のディレクトリで、`cargo build --release` の後)

結果は ../README.md の「実在の小さなクレートをビルドする」。
