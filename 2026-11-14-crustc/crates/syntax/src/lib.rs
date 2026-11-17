//! crustc の構文の層 (コンパイラと LSP・解析で共有する)。
//! ソース → cfg / マクロの展開 / `mod` を平らに / 入れ子のアイテムの移動 (prep) → アイテムへの分割 (lexer::split) →
//! 構文木 (parse) → アイテムの要約 (ast_summary)。prelude はコンパイラが付ける標準ライブラリの代わり。
//! (ファイルの中は `crate::fxhash` のように書く: 下の層のモジュールをここで公開し直す。crustc 自身でコンパイルするときは
//! 全部を 1 つのクレートのモジュールに並べるので、同じパスで引ける)

pub use crustc_base::*;

pub mod ast_closure;
pub mod ast_summary;
pub mod ast_types;
pub mod cfg;
pub mod ctors;
pub mod enc;
pub mod front;
pub mod lexer;
pub mod macros;
pub mod parse;
pub mod prelude;
pub mod prep;
pub mod srcmap;
pub mod stub;
pub mod symbols;
pub mod types;
pub mod types_enc;
