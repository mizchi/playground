//! crustc: インクリメンタル開発ビルド特化の小さな Rust (サブセット) コンパイラ。
//! 中身は crates/ の crate に分かれている (このクレートは CLI と、それらの再輸出):
//! - `minclif`: Cranelift に似た API の小さな x86-64 バックエンド (外部 crate なし)
//! - `crustc-core`: フロントエンド・acg・driver2 (syn・Cranelift なし。crustc 自身でコンパイルできる)
//! - `crustc-clif`: driver2 の Cranelift バックエンド (既定)
//! - `lsp`: driver2 の check を使う LSP サーバ (このクレート)

pub use crustc_core::*;
pub use crustc_clif as clif_backend;
pub use crustc_core::driver2::{Diagnostic, Options2, Report2, Session2};

pub mod lsp;
