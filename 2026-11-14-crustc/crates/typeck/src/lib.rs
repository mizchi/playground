//! crustc の型の検査。acg は構文木と型推論の結果を読みながら型を検査し、命令の列を crustc_ir::Recorder に記録する
//! (check / LSP はここで止める。機械語にするのは crustc-codegen の lower)。cache2 は関数ごとの結果 (Blob) の直列化

pub use crustc_ir as ir;
pub use crustc_sema::*;

pub mod acg;
pub mod cache2;
