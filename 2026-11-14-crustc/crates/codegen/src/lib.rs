//! crustc のコード生成。acg は構文木と型推論の結果から minclif の命令の列を作る (型の検査はここで出る。check は機械語に
//! しないでここで止める)。cache2 は関数ごとの結果 (Blob) の直列化、link はリンカの呼び出し。

pub use crustc_sema::*;
pub use minclif;

pub mod acg;
pub mod cache2;
pub mod link;
