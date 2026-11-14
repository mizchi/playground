//! 回帰監視用ベンチ (criterion)。
//!
//! - lsp/*   : LSP の応答性。didChange → publishDiagnostics までの時間 (in-process)
//! - build/* : ビルド速度。cold / no-op / 1 関数編集 (リンクを含む)
//! - front/* : 字句解析だけ (no-op の下限)
//!
//! 使い方は bench/regress.sh を参照 (ベースライン保存と比較)。
//! リンカは環境変数 EXRUSTC_BENCH_LINKER で固定できる (未指定なら自動選択)。

use std::path::PathBuf;
use std::time::Duration;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use exrustc::lsp::Server;
use exrustc::testgen::program;
use exrustc::{Options, Session};
use serde_json::json;

const N: usize = 1000;

fn opts() -> Options {
    Options {
        linker: std::env::var("EXRUSTC_BENCH_LINKER").ok(),
        ..Options::default()
    }
}

fn open_msg(text: &str) -> String {
    json!({"jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": {"textDocument": {"uri": "file:///b.rs", "languageId": "rust", "version": 1, "text": text}}})
    .to_string()
}

fn change_msg(text: &str) -> String {
    json!({"jsonrpc": "2.0", "method": "textDocument/didChange",
        "params": {"textDocument": {"uri": "file:///b.rs", "version": 2}, "contentChanges": [{"text": text}]}})
    .to_string()
}

fn tmpdir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bench");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn lsp(c: &mut Criterion) {
    let v = [program(N, 0), program(N, 1), program(N, 2)];
    let (c1, c2) = (change_msg(&v[1]), change_msg(&v[2]));
    let mut g = c.benchmark_group("lsp");

    // 1 関数だけ編集した didChange (編集を交互に切り替え、毎回 1 関数が再検査になる)
    let mut srv = Server::new();
    srv.handle(&open_msg(&v[0]));
    let mut k = false;
    g.bench_function(format!("did_change_1fn/{N}"), |b| {
        b.iter(|| {
            k = !k;
            srv.handle(if k { &c1 } else { &c2 })
        })
    });

    // 内容が変わらない didChange (保存だけ、など)
    let same = change_msg(&v[0]);
    let mut srv = Server::new();
    srv.handle(&open_msg(&v[0]));
    g.bench_function(format!("did_change_noop/{N}"), |b| {
        b.iter(|| srv.handle(&same))
    });

    g.finish();
}

/// cold 系はスレッドのスケジューリングやページキャッシュで実行ごとに ±10% 程度ぶれるので、
/// 別グループにして回帰判定の閾値を 15% にする
fn cold(c: &mut Criterion) {
    let v = program(N, 0);
    let mut g = c.benchmark_group("cold");
    g.noise_threshold(0.15);
    // LSP: 初めて開いたとき (全関数を検査)
    let open = open_msg(&v);
    g.bench_function(format!("lsp_did_open/{N}"), |b| {
        b.iter_batched(
            Server::new,
            |mut s| s.handle(&open),
            BatchSize::PerIteration,
        )
    });
    let out = tmpdir().join("cold");
    g.bench_function(format!("build/{N}"), |b| {
        b.iter_batched(
            || Session::new(opts()),
            |mut s| s.build(&v, &out).unwrap(),
            BatchSize::PerIteration,
        )
    });
    g.finish();
}

fn build(c: &mut Criterion) {
    let v = [program(N, 0), program(N, 1), program(N, 2)];
    let dir = tmpdir();
    let mut g = c.benchmark_group("build");

    // CLI と同じく build → save_cache (今回使ったエントリだけを残す) の流れ
    let out = dir.join("noop");
    let cp = exrustc::cache_path(&out);
    let mut s = Session::new(opts());
    s.build(&v[0], &out).unwrap();
    s.save_cache(&cp).unwrap();
    g.bench_function(format!("noop/{N}"), |b| {
        b.iter(|| {
            let r = s.build(&v[0], &out).unwrap();
            s.save_cache(&cp).unwrap();
            assert!(!r.linked);
        })
    });

    let out = dir.join("edit");
    let cp = exrustc::cache_path(&out);
    let mut s = Session::new(opts());
    s.build(&v[1], &out).unwrap();
    s.save_cache(&cp).unwrap();
    let mut k = false;
    g.bench_function(format!("edit_1fn/{N}"), |b| {
        b.iter(|| {
            k = !k;
            let r = s.build(if k { &v[2] } else { &v[1] }, &out).unwrap();
            s.save_cache(&cp).unwrap();
            assert_eq!(r.fns_compiled, 1);
        })
    });
    g.finish();
}

fn front(c: &mut Criterion) {
    let v = program(N, 0);
    c.bench_function(&format!("front/lex/{N}"), |b| {
        b.iter(|| exrustc::lexer::split(&v).unwrap().len())
    });
}

fn config() -> Criterion {
    // ノイズで回帰扱いにならないよう、5% 未満の変化は無視する
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3))
        .noise_threshold(0.05)
        .significance_level(0.01)
}

criterion_group! { name = benches; config = config(); targets = lsp, build, cold, front }
criterion_main!(benches);
