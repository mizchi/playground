//! crustc-lsp: LSP サーバ (stdio)。エディタからは `crustc-lsp` を起動する

fn main() {
    std::process::exit(match crustc_lsp::run(std::io::stdin().lock(), std::io::stdout().lock()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("crustc-lsp: {e}");
            1
        }
    });
}
