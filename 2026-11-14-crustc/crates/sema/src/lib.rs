//! crustc の意味の層 (コンパイラと LSP・解析で共有する)。
//! アイテムの要約 → Program (関数・型・トレイト・impl の表、総称関数のインスタンス) → 本体の型推論 (ast_infer)。

pub use crustc_syntax::*;

pub mod ast_consteval;
pub mod ast_infer;
pub mod program;
pub mod solver;
