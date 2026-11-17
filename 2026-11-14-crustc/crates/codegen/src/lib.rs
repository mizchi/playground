//! crustc のバックエンド側。lower は acg が記録した命令の列 (crustc_ir::Recorder) を minclif に組み立て直して
//! 機械語にする (Cranelift なら crustc-clif に渡す)。link はリンカの呼び出し

pub use crustc_typeck::*;
pub use minclif;

pub mod link;
pub mod lower;
