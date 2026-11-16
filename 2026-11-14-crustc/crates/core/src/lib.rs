//! crustc のフロントエンドとコード生成 (syn・Cranelift に依存しない。crustc 自身でもコンパイルできる)。
//! ソース → lexer / macros / prelude → ast_summary → Program → acg (自前の構文木のコード生成) → minclif の命令の列。
//! 機械語にするのは minclif か、crustc-clif (Cranelift)。driver2 がキャッシュ・並列化・リンクをまとめる

pub use minclif;

pub mod acg;
pub mod ast_closure;
pub mod ast_consteval;
pub mod ast_infer;
pub mod ast_summary;
pub mod ast_types;
pub mod cache2;
pub mod ctors;
pub mod driver2;
pub mod enc;
pub mod fxhash;
pub mod key;
pub mod lexer;
pub mod link;
pub mod macros;
pub mod parse;
pub mod prelude;
pub mod program;
pub mod solver;
pub mod srcmap;
#[doc(hidden)]
pub mod testgen;
pub mod types;
pub mod types_enc;
