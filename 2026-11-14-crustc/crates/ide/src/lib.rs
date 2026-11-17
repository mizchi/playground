//! エディタ向けの解析 (crustc-lsp が使う。コンパイラ (crustc) はこのクレートに依存しない)。
//! 構文木も型検査も使わず、crustc-syntax の字句の列から読む: 編集の途中で壊れたソースでも読める所まで答える。
//!
//! - `symbols`: documentSymbol / workspace/symbol のシンボル (アイテムとメンバ)
//! - `navigate`: ファイルごとの索引 (FileIndex) と、definition / references / hover / completion

pub mod navigate;
pub mod symbols;
