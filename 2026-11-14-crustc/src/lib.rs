//! crustc: インクリメンタル開発ビルド特化の小さな Rust (サブセット) コンパイラ。
//! 中身は crates/ の crate に分かれている (このクレートは CLI と、それらの再輸出):
//! - `minclif`: Cranelift に似た API の小さな x86-64 バックエンド (外部 crate なし)
//! - `crustc-core`: フロントエンド・acg・driver2 (syn・Cranelift なし。crustc 自身でコンパイルできる)
//! - `crustc-clif`: driver2 の Cranelift バックエンド (既定)
//! - `crustc-legacy`: 最初の syn + Cranelift のドライバ (check / LSP と比較の基準)

pub use crustc_core::*;
pub use crustc_clif as clif_backend;
pub use crustc_legacy::{cache, closure, codegen, consteval, driver, elfz, front, infer, lsp};
pub use crustc_legacy::{BuildReport, Diagnostic, Options, Session, cache_path};
