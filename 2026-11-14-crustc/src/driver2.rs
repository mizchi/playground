//! syn と Cranelift を使わないドライバ: ソース → (lexer / macros / prelude) → ast_summary → Program →
//! acg (自前の構文木のコード生成) → minclif の ELF の .o → リンク。
//! driver.rs と同じ手順を、外部クレートに依存しないモジュールだけで行う (crustc 自身でもコンパイルできる)。
//! キャッシュと並列化はまだない。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::acg::{self, Blob, ItemFn, Target};
use crate::fxhash::{FxMap, FxSet};
use crate::lexer::{self, ItemKind, SrcItem};
use crate::minclif::{DataDef, Reloc};
use crate::parse::{Block, Expr, Pat, Stmt};
use crate::program::{Insts, Program, TyEnv};
use crate::types::{Summary, Ty};

pub type R<T> = Result<T, String>;

/// ユーザのソースの後ろに、使われうる prelude のアイテムを付ける (driver::with_prelude と同じ)。
/// 返り値は (全体のソース, アイテム, ユーザのアイテムの数)
pub fn with_prelude(src: &str) -> R<(String, Vec<SrcItem>, usize)> {
    let tl = lexer::expand_thread_local(src);
    let src: &str = tl.as_deref().unwrap_or(src);
    let expanded = crate::macros::expand(src)?.or(tl.clone());
    let src: &str = expanded.as_deref().unwrap_or(src);
    let flat = lexer::flatten_modules(src).or(expanded.clone());
    let src: &str = flat.as_deref().unwrap_or(src);
    let hoisted = lexer::hoist_nested_items(src).or(flat.clone());
    let src: &str = hoisted.as_deref().unwrap_or(src);
    let cv = crate::ctors::rewrite_ctor_values(src);
    let src: &str = cv.as_deref().unwrap_or(src);
    let mut items = lexer::split(src)?;
    let user = items.len();
    let mut cache = crate::prelude::SelectCache::default();
    let extra = crate::prelude::select_with(src, &items, &mut cache);
    if extra.is_empty() {
        return Ok((src.to_string(), items, user));
    }
    let base = src.len() + 1;
    let lines = src.bytes().filter(|&c| c == b'\n').count() + 1;
    let n0 = items.len() as u32;
    for mut it in lexer::split(&extra)? {
        it.start += base;
        it.end += base;
        it.line += lines;
        for r in &mut it.idents {
            r.0 += base as u32;
            r.1 += base as u32;
        }
        it.kind = match it.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember(h + n0),
            ItemKind::TraitMember(h) => ItemKind::TraitMember(h + n0),
            k => k,
        };
        items.push(it);
    }
    let mut full = String::with_capacity(base + extra.len());
    full.push_str(src);
    full.push('\n');
    full.push_str(&extra);
    Ok((full, items, user))
}

/// 解析の結果
pub struct Analysis {
    pub src: String,
    pub items: Vec<SrcItem>,
    pub prog: Program,
    pub refs: Vec<Vec<u32>>,
}

pub fn analyze(src0: &str, opt: bool) -> R<Analysis> {
    let (src, items, user) = with_prelude(src0)?;
    if let Some(p) = std::env::var_os("CRUSTC_DUMP_SRC") {
        let _ = std::fs::write(p, &src);
    }
    let mut sums: Vec<Summary> = vec![Summary::default(); items.len()];
    for pass in 0..2 {
        for (i, it) in items.iter().enumerate() {
            let header = matches!(it.kind, ItemKind::ImplHeader | ItemKind::TraitHeader);
            if header != (pass == 0) {
                continue;
            }
            let ow = if pass == 1 { crate::ast_summary::owner_of(it, &sums) } else { None };
            sums[i] = crate::ast_summary::parse_and_summarize(&src, it, ow.as_ref()).map_err(|e| relocate(e, it))?;
        }
    }
    let mut prog = Program::build(&items, &sums)?;
    prog.prelude_from = user as u32;
    prog.overflow_checks = !opt;
    prog.opt = opt;
    resolve_opaque(&mut prog, &src, &items)?;
    let mut refs = Vec::with_capacity(prog.item_of.len());
    for ii in &prog.item_of {
        let (f, _, _) = ii.map(|ii| prog.refs(&src, &items[ii as usize])).unwrap_or_default();
        refs.push(f);
    }
    Ok(Analysis { src, items, prog, refs })
}

/// `-> impl Iterator<..>` の戻り値を本体の型推論で決める (front.rs の resolve_opaque と同じ)
fn resolve_opaque(prog: &mut Program, src: &str, items: &[SrcItem]) -> R<()> {
    if prog.opaque.is_empty() {
        return Ok(());
    }
    let ids = prog.opaque.clone();
    let mut done = vec![false; ids.len()];
    for _ in 0..4 {
        let mut progress = false;
        for (k, &id) in ids.iter().enumerate() {
            if done[k] {
                continue;
            }
            let sig = prog.sigs[id as usize].clone();
            if sig.is_generic() {
                return Err(format!("`{}`: generic functions returning `impl Trait` are not supported", prog.names[id as usize]));
            }
            let Some(ii) = prog.item_of[id as usize] else { continue };
            let Some(f) = parse_fn(src, &items[ii as usize])? else { continue };
            let env = TyEnv { self_sid: sig.impl_of, params: vec![], self_ty: sig.self_ty };
            let (_, r, _) = crate::ast_infer::infer_ctx(prog, f.sig.recv.is_some(), &f.sig.params, &f.block, &sig, &env, &[], None, Some((id, 0)));
            if let Some(t) = r.filter(|&t| t != Ty::Unit && !prog.has_param(t)) {
                prog.sigs[id as usize].ret = t;
                done[k] = true;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    if let Some(k) = done.iter().position(|d| !*d) {
        return Err(format!("cannot infer the `impl Trait` return type of `{}`", prog.names[ids[k] as usize]));
    }
    Ok(())
}

/// アイテムの中の `行:桁:` のエラーを、ファイルの中の行にする (driver.rs の relocate_err と同じ)。位置がなければアイテムの先頭の行
fn relocate(e: String, it: &SrcItem) -> String {
    if let Some((l, rest)) = e.split_once(':') {
        if let Ok(l) = l.parse::<usize>() {
            return format!("{}:{rest}", l + it.line - 1);
        }
    }
    format!("{}:1: {e}", it.line)
}

/// アイテムの関数を自前の構文木で読む (本体がなければ None)。引数のパターンは本体の先頭の let にする
pub fn parse_fn(src: &str, it: &SrcItem) -> R<Option<ItemFn>> {
    let text = &src[it.start..it.end];
    let f = crate::parse::parse_file(text).map_err(|e| relocate(e, it))?;
    let Some(item) = f.items.into_iter().next() else { return Ok(None) };
    let crate::parse::ItemKind::Fn(mut sig, Some(mut block)) = item.kind else { return Ok(None) };
    let mut lets = vec![];
    for (k, (p, _)) in sig.params.iter_mut().enumerate() {
        if matches!(p, Pat::Ident(..) | Pat::Wild) {
            continue;
        }
        let name = format!("__exr_arg{k}");
        let pat = std::mem::replace(p, Pat::Ident(false, false, name.clone(), None));
        lets.push(Stmt::Let(pat, None, Some(acg::path_expr(&name)), None));
    }
    if !lets.is_empty() {
        lets.append(&mut block.stmts);
        block.stmts = lets;
    }
    Ok(Some(ItemFn { attrs: item.attrs, sig, block }))
}

/// クロージャ式を関数にする (`fn call(__env: (), p0: (), ..) { 本体 }`: 型はシグネチャから決まる)
pub fn synth_closure(c: &Expr) -> ItemFn {
    let Expr::Closure(_, ps, _, body) = c else { unreachable!() };
    let unit = crate::parse::Ty::Tuple(vec![]);
    let mut params = vec![(Pat::Ident(false, false, "__env".into(), None), unit.clone())];
    let mut stmts = vec![];
    for (i, (p, _)) in ps.iter().enumerate() {
        let simple = matches!(p, Pat::Ident(false, _, _, None));
        let pin = if simple {
            p.clone()
        } else {
            let id = format!("__p{i}");
            stmts.push(Stmt::Let(p.clone(), None, Some(acg::path_expr(&id)), None));
            Pat::Ident(false, false, id, None)
        };
        params.push((pin, unit.clone()));
    }
    match &**body {
        Expr::Block(None, false, b) => stmts.extend(b.stmts.iter().cloned()),
        e => stmts.push(Stmt::Expr(e.clone(), false)),
    }
    let sig = crate::parse::FnSig {
        name: "call".into(),
        generics: crate::parse::Generics { params: vec![], preds: vec![] },
        recv: None,
        recv_ty: None,
        params,
        ret: None,
        quals: vec![],
    };
    ItemFn { attrs: vec![], sig, block: Block { stmts } }
}

/// 関数 (インスタンスも) id の本体
fn fn_ast(a: &Analysis, body: u32) -> R<ItemFn> {
    let ii = a.prog.item_of[body as usize].ok_or("internal: no item")?;
    let it = &a.items[ii as usize];
    let f = parse_fn(&a.src, it)?.ok_or_else(|| format!("`{}` has no body", a.prog.names[body as usize]))?;
    match a.prog.closures.get(&body) {
        Some(cd) => {
            let cs = crate::ast_closure::closures_of(&f.block);
            let c = cs.get(cd.k as usize).ok_or("internal: closure not found")?;
            Ok(synth_closure(c))
        }
        None => Ok(f),
    }
}

fn concrete(p: &Program, id: usize) -> bool {
    p.item_of[id].is_some() && !p.sigs[id].is_generic() && p.sigs[id].has_body
}

/// ソースから ELF の .o を作る
pub fn build_object(src0: &str, opt: bool) -> R<Vec<u8>> {
    let a = analyze(src0, opt)?;
    let mut live = a.prog.reachable(&a.refs);
    let mut user_live: Vec<u32> = (0..a.prog.names.len()).filter(|&i| live[i] && concrete(&a.prog, i)).map(|i| i as u32).collect();
    let insts = Mutex::new(Insts::new(&a.prog));
    let mut blobs: FxMap<u32, Blob> = FxMap::default();
    let mut wave = user_live.clone();
    while !wave.is_empty() {
        for &id in &wave {
            if blobs.contains_key(&id) {
                continue;
            }
            let body = insts.lock().unwrap().body_of(id);
            let f = fn_ast(&a, body)?;
            let ii = a.prog.item_of[body as usize].unwrap() as usize;
            let it = &a.items[ii];
            // (acg のエラーはファイルの中の `行:桁:` を持つ。持たないものはアイテムの先頭の行にする)
            let (b, _) = acg::compile(&a.prog, &insts, id, &f, it.line as u32 - 1)
                .map_err(|e| if acg::has_loc(&e) { e } else { format!("{}:1: in `{}`: {e}", it.line, a.prog.names[body as usize]) })?;
            blobs.insert(id, b);
        }
        wave = std::mem::take(&mut insts.lock().unwrap().pending);
        wave.retain(|id| !blobs.contains_key(id));
        let starts: Vec<u32> = {
            let g = insts.lock().unwrap();
            wave.iter().map(|&id| g.body_of(id)).collect()
        };
        let mut stack: Vec<u32> = starts.iter().flat_map(|&b| a.refs[b as usize].iter().copied()).collect();
        if starts.iter().any(|&b| matches!(a.prog.names[b as usize].as_str(), "__exr_display" | "__exr_debug")) {
            stack.extend((0..a.prog.names.len() as u32).filter(|&i| matches!(a.prog.sigs[i as usize].impl_trait.as_deref(), Some("Display" | "Debug"))));
        }
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut live[i as usize], true) {
                continue;
            }
            stack.extend(a.refs[i as usize].iter().copied());
            if concrete(&a.prog, i as usize) && !blobs.contains_key(&i) {
                user_live.push(i);
                wave.push(i);
            }
        }
    }
    let insts = insts.into_inner().unwrap();
    emit_object(&a.prog, &insts, &user_live, &blobs)
}

/// ソースから実行ファイルを作る
pub fn build(src: &str, output: &std::path::Path, opt: bool, linker: Option<&str>) -> R<()> {
    let obj = build_object(src, opt)?;
    let obj_path = output.with_extension("o");
    std::fs::write(&obj_path, &obj).map_err(|e| e.to_string())?;
    crate::link::link(linker, obj_path.as_path(), output)?;
    Ok(())
}

fn emit_object(prog: &Program, insts: &Insts, user_live: &[u32], blobs: &FxMap<u32, Blob>) -> R<Vec<u8>> {
    let sym_of = |id: u32| -> &str {
        match insts.get(id) {
            Some(i) => &i.sig.symbol,
            None => &prog.sigs[id as usize].symbol,
        }
    };
    // 置く関数: main / export から、再配置と vtable を辿って届くものだけ
    let mut reach: FxSet<u32> = FxSet::default();
    let mut stack: Vec<u32> = user_live
        .iter()
        .copied()
        .filter(|&u| {
            let s = &prog.sigs[u as usize];
            s.is_main || s.export
        })
        .collect();
    while let Some(u) = stack.pop() {
        if !reach.insert(u) {
            continue;
        }
        let Some(b) = blobs.get(&u) else { continue };
        for r in &b.relocs {
            if let Target::Func(f) = r.target {
                stack.push(f);
            }
        }
        for vt in &b.vtables {
            stack.extend(vt.iter().copied().filter(|&f| f != u32::MAX));
        }
    }
    let mut placed: Vec<u32> = user_live.iter().copied().filter(|u| reach.contains(u) && blobs.contains_key(u)).collect();
    let mut order: Vec<u32> = (0..insts.list.len() as u32).map(|i| insts.n_fns + i).filter(|i| reach.contains(i) && blobs.contains_key(i)).collect();
    order.sort_by(|a, b| sym_of(*a).cmp(sym_of(*b)));
    // 同じ機械語になるインスタンスは 1 つだけ置く (ICF)
    let mut fold: FxMap<u32, u32> = FxMap::default();
    for _ in 0..4 {
        let canon = |f: u32, fold: &FxMap<u32, u32>| *fold.get(&f).unwrap_or(&f);
        let mut seen: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut changed = false;
        for &iid in &order {
            if fold.contains_key(&iid) {
                continue;
            }
            let b = &blobs[&iid];
            let sig = &insts.get(iid).unwrap().sig;
            let mut key = Vec::with_capacity(b.code.len() + 64);
            key.extend_from_slice(&(b.code.len() as u32).to_le_bytes());
            key.extend_from_slice(&b.code);
            key.extend_from_slice(format!("{:?}", acg::clif_sig(sig)).as_bytes());
            for r in &b.relocs {
                key.extend_from_slice(&r.offset.to_le_bytes());
                key.push(r.kind);
                key.extend_from_slice(&r.addend.to_le_bytes());
                match r.target {
                    Target::Func(f) => {
                        key.push(0);
                        key.extend_from_slice(&canon(f, &fold).to_le_bytes());
                    }
                    Target::SelfOffset(o) => {
                        key.push(1);
                        key.extend_from_slice(&o.to_le_bytes());
                    }
                    Target::Data(k) => {
                        key.push(2);
                        key.extend_from_slice(&b.datas[k as usize]);
                        key.push(0xFF);
                    }
                    Target::Static(c) => {
                        key.push(3);
                        key.extend_from_slice(&c.to_le_bytes());
                    }
                    Target::Sym(ref n) => {
                        key.push(5);
                        key.extend_from_slice(n.as_bytes());
                    }
                    Target::Vtable(k) => {
                        key.push(4);
                        for &f in &b.vtables[k as usize] {
                            key.extend_from_slice(&canon(f, &fold).to_le_bytes());
                        }
                        key.push(0xFF);
                    }
                }
            }
            for &(o, i, mi) in &b.traps {
                key.extend_from_slice(&o.to_le_bytes());
                key.extend_from_slice(&i.to_le_bytes());
                if mi != u32::MAX {
                    key.extend_from_slice(&b.datas[mi as usize]);
                }
                key.push(0xFE);
            }
            match seen.get(&key) {
                Some(&r) => {
                    fold.insert(iid, r);
                    changed = true;
                }
                None => {
                    seen.insert(key, iid);
                }
            }
        }
        if !changed {
            break;
        }
    }
    order.retain(|i| !fold.contains_key(i));
    let resolve = |mut f: u32| {
        while let Some(&n) = fold.get(&f) {
            f = n;
        }
        f
    };
    placed.extend(order.iter().copied());
    let fname = |f: u32| -> String {
        let f = resolve(f);
        sym_of(f).to_string()
    };
    // static: コードから参照されているもの (と、その初期値から参照されているもの)
    let mut datas: Vec<DataDef> = vec![];
    let mut anon: HashMap<Vec<u8>, String> = HashMap::new();
    let mut anon_data = |bytes: &[u8], datas: &mut Vec<DataDef>| -> String {
        if let Some(n) = anon.get(&bytes.to_vec()) {
            return n.clone();
        }
        let n = format!(".Lxd{}", anon.len());
        anon.insert(bytes.to_vec(), n.clone());
        datas.push((n.clone(), bytes.to_vec(), vec![], false));
        n
    };
    let static_name = |ci: u32| format!("_xS{}", prog.consts[ci as usize].name);
    let mut todo: Vec<u32> = vec![];
    for u in &placed {
        for r in &blobs[u].relocs {
            if let Target::Static(c) = r.target {
                todo.push(c);
            }
        }
    }
    todo.sort_unstable();
    todo.dedup();
    let mut done_static: FxSet<u32> = FxSet::default();
    let mut trap_table: Option<u32> = None;
    let mut nested = 0usize;
    while let Some(ci) = todo.pop() {
        if !done_static.insert(ci) {
            continue;
        }
        let c = &prog.consts[ci as usize];
        if c.name == "__EXR_TRAP_TABLE" {
            trap_table = Some(ci);
            continue;
        }
        let dv = crate::ast_consteval::eval_src(prog, &c.expr, c.ty).map_err(|e| format!("in static `{}`: {e}", c.name))?;
        // (入れ子の無名データ (`&[..]` の中身) は名前を付けて置く)
        fn place(
            dv: crate::ast_consteval::DataVal,
            name: String,
            datas: &mut Vec<DataDef>,
            todo: &mut Vec<u32>,
            nested: &mut usize,
            static_name: &dyn Fn(u32) -> String,
        ) {
            let mut rs = vec![];
            for (off, r) in dv.relocs {
                match r {
                    crate::ast_consteval::DRef::Static(c) => {
                        todo.push(c);
                        rs.push((off, static_name(c), 0));
                    }
                    crate::ast_consteval::DRef::Anon(inner) => {
                        *nested += 1;
                        let n = format!(".Lxn{}", *nested);
                        place(inner, n.clone(), datas, todo, nested, static_name);
                        rs.push((off, n, 0));
                    }
                }
            }
            datas.push((name, dv.bytes, rs, false));
        }
        place(dv, static_name(ci), &mut datas, &mut todo, &mut nested, &static_name);
    }
    // 関数とその再配置
    let mut funcs: Vec<(String, Vec<u8>, Vec<Reloc>, bool)> = vec![];
    let mut vts: HashMap<Vec<u32>, String> = HashMap::new();
    let mut fn_off: Vec<(u32, u64)> = vec![];
    let mut text_end = 0u64;
    for &uid in &placed {
        let blob = &blobs[&uid];
        let data_names: Vec<String> = blob.datas.iter().map(|d| anon_data(d, &mut datas)).collect();
        let mut vt_names = vec![];
        for vt in &blob.vtables {
            if let Some(n) = vts.get(vt) {
                vt_names.push(n.clone());
                continue;
            }
            let n = format!(".Lxv{}", vts.len());
            let mut rs = vec![];
            for (i, &f) in vt.iter().enumerate() {
                if f != u32::MAX && (reach.contains(&resolve(f)) || prog.sigs.get(f as usize).is_some_and(|s| s.is_extern)) {
                    rs.push((8 * i as u32, fname(f), 0));
                }
            }
            let size: usize = 8 * vt.len().max(1);
            datas.push((n.clone(), vec![0u8; size], rs, false));
            vts.insert(vt.clone(), n.clone());
            vt_names.push(n);
        }
        let relocs: Vec<Reloc> = blob
            .relocs
            .iter()
            .map(|r| Reloc {
                offset: r.offset,
                name: match r.target {
                    Target::Func(t) => fname(t),
                    Target::SelfOffset(_) => sym_of(uid).to_string(),
                    Target::Data(k) => data_names[k as usize].clone(),
                    Target::Static(c) => static_name(c),
                    Target::Vtable(k) => vt_names[k as usize].clone(),
                    Target::Sym(ref n) => n.clone(),
                },
                kind: if r.kind == 5 { 9 } else { 4 },
                addend: match r.target {
                    Target::SelfOffset(o) => r.addend + o as i64,
                    _ => r.addend,
                },
            })
            .collect();
        let s = &prog.sigs.get(uid as usize);
        let global = insts.get(uid).is_none() && s.is_some_and(|s| s.is_main || s.export);
        text_end = (text_end + 15) / 16 * 16;
        fn_off.push((uid, text_end));
        text_end += blob.code.len() as u64;
        funcs.push((sym_of(uid).to_string(), blob.code.clone(), relocs, global));
    }
    // panic の trap の表 (driver.rs と同じ形式)
    if let Some(ci) = trap_table {
        let mut bytes: Vec<u8> = vec![0; 8];
        let uv = |out: &mut Vec<u8>, mut v: u64| {
            while v >= 0x80 {
                out.push(v as u8 | 0x80);
                v >>= 7;
            }
            out.push(v as u8);
        };
        let mut msgs: Vec<Vec<u8>> = vec![];
        let mut fbytes: Vec<u8> = vec![];
        let mut nf = 0u64;
        let (mut prev_fn, mut prev_line) = (0u64, 0i64);
        for &(uid, fo) in &fn_off {
            let blob = &blobs[&uid];
            if blob.traps.is_empty() {
                continue;
            }
            nf += 1;
            uv(&mut fbytes, fo - prev_fn);
            prev_fn = fo;
            uv(&mut fbytes, blob.traps.len() as u64);
            let mut prev_site = 0u64;
            for &(off, info, mi) in &blob.traps {
                let gm = if mi == u32::MAX {
                    0
                } else {
                    let b = &blob.datas[mi as usize];
                    match msgs.iter().position(|x| x == b) {
                        Some(k) => k as u64 + 1,
                        None => {
                            msgs.push(b.clone());
                            msgs.len() as u64
                        }
                    }
                };
                let (kind, line, col) = ((info >> 27) as u64, ((info >> 11) & 0xFFFF) as i64, (info & 0x7FF) as u64);
                uv(&mut fbytes, off as u64 - prev_site);
                prev_site = off as u64;
                uv(&mut fbytes, kind | (gm << 4));
                let d = line - prev_line;
                prev_line = line;
                uv(&mut fbytes, ((d << 1) ^ (d >> 63)) as u64);
                uv(&mut fbytes, col);
            }
        }
        uv(&mut bytes, msgs.len() as u64);
        for mb in &msgs {
            uv(&mut bytes, mb.len() as u64);
            bytes.extend_from_slice(mb);
        }
        uv(&mut bytes, nf);
        bytes.extend_from_slice(&fbytes);
        let rs = match funcs.first() {
            Some(f) => vec![(0u32, f.0.clone(), 0i64)],
            None => vec![],
        };
        datas.push((static_name(ci), bytes, rs, false));
    }
    Ok(crate::minclif::write_object_full(&funcs, &datas))
}
