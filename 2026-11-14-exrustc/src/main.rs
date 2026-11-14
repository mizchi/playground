//! exrustc: インクリメンタル開発ビルド特化の小さな Rust (サブセット) コンパイラ。
//!
//! パイプライン:
//!   lex (自前) → 変わったアイテムだけ syn で要約 → 到達可能性 → 関数ごとのキャッシュキー
//!   → ミスした関数だけ parse + 型検査 + Cranelift → 1 個の .o に詰める → リンク (変化がなければ省略)

mod cache;
mod codegen;
mod front;
mod infer;
mod lexer;
mod link;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use cranelift_codegen::isa::{OwnedTargetIsa, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_module::{Linkage, Module, ModuleReloc, ModuleRelocTarget};
use cranelift_object::{ObjectBuilder, ObjectModule};

use cache::{Cache, Key, KeyHasher};
use codegen::{Blob, RELOC_KINDS, Target, Worker};
use front::{Program, Summary};
use lexer::SrcItem;

struct Opts {
    input: PathBuf,
    output: PathBuf,
    opt: bool,
    fast_regalloc: bool,
    linker: Option<String>,
    time: bool,
    jobs: usize,
    no_cache: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: exrustc <input.rs> [-o out] [-O] [--regalloc=fast|good] [--linker=NAME] [--time] [-j N] [--no-cache]"
    );
    std::process::exit(2)
}

fn parse_args() -> Opts {
    let mut args = std::env::args().skip(1);
    let mut o = Opts {
        input: PathBuf::new(),
        output: PathBuf::new(),
        opt: false,
        fast_regalloc: true,
        linker: None,
        time: false,
        jobs: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
        no_cache: false,
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "-o" => o.output = args.next().unwrap_or_else(|| usage()).into(),
            "-O" => {
                o.opt = true;
                o.fast_regalloc = false;
            }
            "--regalloc=fast" => o.fast_regalloc = true,
            "--regalloc=good" => o.fast_regalloc = false,
            "--time" => o.time = true,
            "--no-cache" => o.no_cache = true,
            "-j" => o.jobs = args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()),
            s if s.starts_with("--linker=") => o.linker = Some(s["--linker=".len()..].into()),
            s if s.starts_with('-') => usage(),
            s => o.input = s.into(),
        }
    }
    if o.input.as_os_str().is_empty() {
        usage();
    }
    if o.output.as_os_str().is_empty() {
        o.output = o.input.with_extension("");
    }
    o
}

struct Timer {
    on: bool,
    start: Instant,
    last: Instant,
}
impl Timer {
    fn lap(&mut self, what: &str, extra: impl std::fmt::Display) {
        if self.on {
            let now = Instant::now();
            eprintln!("  {:<10} {:>8.2} ms  {}", what, (now - self.last).as_secs_f64() * 1e3, extra);
            self.last = now;
        }
    }
}

fn make_isa(o: &Opts) -> OwnedTargetIsa {
    let mut f = settings::builder();
    f.set("opt_level", if o.opt { "speed_and_size" } else { "none" }).unwrap();
    f.set("regalloc_algorithm", if o.fast_regalloc { "single_pass" } else { "backtracking" })
        .unwrap();
    f.set("is_pic", "true").unwrap();
    // 開発ビルドではアンウィンド情報不要 (panic/unwind をまだ持たない)
    f.set("unwind_info", "false").unwrap();
    cranelift_native::builder()
        .unwrap()
        .finish(settings::Flags::new(f))
        .unwrap()
}

/// コンパイラ自身の同一性。実行ファイルのサイズと mtime を使う
/// (コンパイラを作り直したらキャッシュは全て無効になる。安全側)。
fn compiler_id() -> (u64, u128) {
    let exe = std::env::current_exe().ok();
    let md = exe.and_then(|p| std::fs::metadata(p).ok());
    let len = md.as_ref().map(|m| m.len()).unwrap_or(0);
    let mt = md
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (len, mt)
}

fn main() {
    let o = parse_args();
    if let Err(e) = run(&o) {
        eprintln!("error: {}: {e}", o.input.display());
        std::process::exit(1);
    }
}

fn run(o: &Opts) -> Result<(), String> {
    let t0 = Instant::now();
    let mut tm = Timer { on: o.time, start: t0, last: t0 };

    let src = std::fs::read_to_string(&o.input).map_err(|e| e.to_string())?;
    let items = lexer::split(&src)?;
    tm.lap("lex", format!("{} bytes, {} items", src.len(), items.len()));

    let cache_path = cache_path(&o.output);
    let cache = if o.no_cache { Cache::empty() } else { Cache::open(&cache_path) };
    let isa = make_isa(o);
    let cid = compiler_id();

    // --- アイテム要約: 変わったアイテムだけ syn で parse --------------------
    let mut parsed: HashMap<usize, syn::Item> = HashMap::new();
    let mut sums = Vec::with_capacity(items.len());
    let mut sum_keys = Vec::with_capacity(items.len());
    for (ii, it) in items.iter().enumerate() {
        let mut h = KeyHasher::new();
        h.add(&(cid, 0u8, it.hash));
        let k = h.finish();
        sum_keys.push(k);
        let cached = cache.get(k).and_then(Summary::decode);
        let sm = match cached {
            Some(sm) => sm,
            None => {
                let item = front::parse_item(&src, it)?;
                let sm = front::summarize(&item).map_err(|e| front::relocate_err(e, it))?;
                parsed.insert(ii, item);
                sm
            }
        };
        sums.push(sm);
    }
    let prog = Program::build(&items, &sums)?;
    tm.lap("summary", format!("{} parsed / {} cached", parsed.len(), items.len() - parsed.len()));

    // --- 到達可能性とキャッシュキー ------------------------------------------
    let refs: Vec<Vec<u32>> = prog
        .item_of
        .iter()
        .map(|ii| ii.map(|ii| prog.refs(&src, &items[ii as usize])).unwrap_or_default())
        .collect();
    let live = prog.reachable(&refs);
    let mut base = KeyHasher::new();
    base.add(&(cid, 1u8));
    base.add(isa.triple().to_string().as_str());
    base.add(&(o.opt, o.fast_regalloc));
    let base = base.finish();
    let mut keys: Vec<Key> = vec![0; prog.names.len()];
    let mut user_live = vec![];
    for (id, ii) in prog.item_of.iter().enumerate() {
        let Some(ii) = ii else { continue };
        if !live[id] {
            continue;
        }
        user_live.push(id as u32);
        let mut h = KeyHasher::new();
        h.add(&(base, items[*ii as usize].hash));
        // 参照先はシグネチャだけ (本体の変更は呼び出し元に波及しない)
        for &r in &refs[id] {
            h.add(prog.names[r as usize].as_str());
            h.add(&prog.sigs[r as usize]);
        }
        keys[id] = h.finish();
    }
    let n_user = prog.item_of.iter().filter(|i| i.is_some()).count();
    tm.lap("plan", format!("{} live fns, {} unreachable skipped", user_live.len(), n_user - user_live.len()));

    // --- キャッシュ参照 -------------------------------------------------
    let mut blobs: HashMap<u32, Blob> = HashMap::new();
    let mut misses = vec![];
    for &id in &user_live {
        match cache.get(keys[id as usize]).and_then(|d| cache::decode_blob(d, &prog.index)) {
            Some(b) => {
                blobs.insert(id, b);
            }
            None => misses.push(id),
        }
    }
    tm.lap(
        "cache",
        format!("{} hit / {} miss (cache {} entries, {} bytes)", blobs.len(), misses.len(), cache.len(), cache.file_size()),
    );

    // --- ミスした関数だけ parse + 型検査 + Cranelift ----------------------
    let compiled = compile_all(o, &src, &items, &prog, &misses, parsed, &isa)?;
    blobs.extend(compiled);
    tm.lap("codegen", format!("{} fns", misses.len()));

    // --- 1 個のオブジェクトに詰める --------------------------------------
    let obj = emit_object(&prog, &live, &user_live, &blobs, isa.clone())?;
    let mut oh = KeyHasher::new();
    oh.add(&obj[..]);
    oh.add(&o.linker); // リンカを変えたら再リンク
    let obj_key = oh.finish();
    tm.lap("object", format!("{} bytes", obj.len()));

    // --- キャッシュ書き戻し (今回使ったものだけ = 自動 GC) -------------------
    if !o.no_cache {
        let mut bufs: Vec<(Key, Vec<u8>)> = Vec::with_capacity(items.len() + user_live.len());
        for (k, sm) in sum_keys.iter().zip(&sums) {
            let mut b = vec![];
            sm.encode(&mut b);
            bufs.push((*k, b));
        }
        for &id in &user_live {
            let mut b = vec![];
            cache::encode_blob(&blobs[&id], &prog.names, &mut b);
            bufs.push((keys[id as usize], b));
        }
        let entries: Vec<(Key, &[u8])> = bufs.iter().map(|(k, b)| (*k, &b[..])).collect();
        let sz = cache.save(&cache_path, &entries, obj_key).map_err(|e| e.to_string())?;
        tm.lap("cache-save", format!("{} bytes", sz));
    }

    // --- リンク (オブジェクトが前回と同一なら省略) ----------------------------
    let obj_path = o.output.with_extension("o");
    if !o.no_cache && obj_key == cache.last_obj && o.output.exists() && obj_path.exists() {
        tm.lap("link", "skipped (object unchanged)");
    } else {
        std::fs::write(&obj_path, &obj).map_err(|e| e.to_string())?;
        let used = link::link(o.linker.as_deref(), &obj_path, &o.output)?;
        let sz = std::fs::metadata(&o.output).map(|m| m.len()).unwrap_or(0);
        tm.lap("link", format!("{used} -> {sz} bytes"));
    }
    if o.time {
        eprintln!(
            "  {:<10} {:>8.2} ms  peak RSS {} KiB",
            "total",
            tm.start.elapsed().as_secs_f64() * 1e3,
            peak_rss_kib()
        );
    }
    Ok(())
}

fn cache_path(output: &Path) -> PathBuf {
    let mut s = output.as_os_str().to_owned();
    s.push(".exrc");
    PathBuf::from(s)
}

fn compile_one(
    w: &mut Worker,
    src: &str,
    items: &[SrcItem],
    prog: &Program,
    id: u32,
    pre: Option<syn::Item>,
    isa: &dyn TargetIsa,
) -> Result<Blob, String> {
    let it = &items[prog.item_of[id as usize].unwrap() as usize];
    let item = match pre {
        Some(i) => i,
        None => front::parse_item(src, it)?,
    };
    let syn::Item::Fn(f) = item else { unreachable!() };
    w.compile(prog, id, &f, isa).map_err(|e| front::relocate_err(e, it))
    // f (AST) はここで捨てる: 同時に生きている AST は常に 1 関数分だけ
}

fn compile_all(
    o: &Opts,
    src: &str,
    items: &[SrcItem],
    prog: &Program,
    misses: &[u32],
    mut parsed: HashMap<usize, syn::Item>,
    isa: &OwnedTargetIsa,
) -> Result<Vec<(u32, Blob)>, String> {
    // ミスが少ない (= インクリメンタル) ときはスレッドを起動しない。
    // syn の AST は !Send なので、並列時は各スレッドが担当アイテムだけを parse する。
    let jobs = o.jobs.min(misses.len() / 32).max(1);
    if jobs == 1 {
        let mut w = Worker::new();
        return misses
            .iter()
            .map(|&id| {
                let pre = parsed.remove(&(prog.item_of[id as usize].unwrap() as usize));
                Ok((id, compile_one(&mut w, src, items, prog, id, pre, &**isa)?))
            })
            .collect();
    }
    drop(parsed);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..jobs)
            .map(|_| {
                let isa: Arc<dyn TargetIsa> = isa.clone();
                let next = &next;
                s.spawn(move || -> Result<Vec<(u32, Blob)>, String> {
                    let mut w = Worker::new();
                    let mut out = vec![];
                    loop {
                        // 小さな塊ごとに取りに行く (work stealing 的に負荷を均す)
                        let i = next.fetch_add(8, std::sync::atomic::Ordering::Relaxed);
                        if i >= misses.len() {
                            break;
                        }
                        for &id in &misses[i..(i + 8).min(misses.len())] {
                            out.push((id, compile_one(&mut w, src, items, prog, id, None, &*isa)?));
                        }
                    }
                    Ok(out)
                })
            })
            .collect();
        let mut out = vec![];
        for h in hs {
            out.extend(h.join().map_err(|_| "worker panicked".to_string())??);
        }
        Ok(out)
    })
}

fn emit_object(
    prog: &Program,
    live: &[bool],
    user_live: &[u32],
    blobs: &HashMap<u32, Blob>,
    isa: OwnedTargetIsa,
) -> Result<Vec<u8>, String> {
    let cc = isa.default_call_conv();
    let mut b = ObjectBuilder::new(isa, "exr", cranelift_module::default_libcall_names())
        .map_err(|e| e.to_string())?;
    // 関数ごとにセクションを分ける → リンカの --gc-sections が効く
    b.per_function_section(true);
    let mut m = ObjectModule::new(b);
    let mut ids = HashMap::new();
    for (i, sig) in prog.sigs.iter().enumerate() {
        if !live[i] {
            continue;
        }
        let linkage = if sig.is_extern {
            Linkage::Import
        } else if sig.is_main {
            Linkage::Export
        } else {
            Linkage::Local
        };
        let id = m
            .declare_function(&sig.symbol, linkage, &codegen::clif_sig(sig, cc))
            .map_err(|e| e.to_string())?;
        ids.insert(i as u32, id);
    }
    for &uid in user_live {
        let blob = &blobs[&uid];
        let fid = ids[&uid];
        let relocs: Vec<ModuleReloc> = blob
            .relocs
            .iter()
            .map(|r| ModuleReloc {
                offset: r.offset,
                kind: RELOC_KINDS[r.kind as usize],
                addend: r.addend,
                name: match r.target {
                    Target::Func(t) => ModuleRelocTarget::from(ids[&t]),
                    Target::SelfOffset(off) => ModuleRelocTarget::FunctionOffset(fid, off),
                },
            })
            .collect();
        m.define_function_bytes(fid, blob.align as u64, &blob.code, &relocs)
            .map_err(|e| e.to_string())?;
    }
    m.finish().emit().map_err(|e| e.to_string())
}

fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}
