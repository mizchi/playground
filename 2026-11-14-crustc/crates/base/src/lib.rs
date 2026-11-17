//! crustc の全体で使う小さな部品 (外部クレートに依存しない。crustc 自身でもコンパイルできる)。
//! - fxhash: 名前表などの FxHasher / FxMap / FxSet
//! - key: キャッシュのキー (Key / KeyHasher)
//! - prof: 段ごとの時間の計測

pub mod fxhash;
pub mod key;
pub mod prof;
