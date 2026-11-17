//! フロントエンドの流れ (コンパイラと LSP・解析で共有する): 前処理 (prep) → アイテムへの分割 (split、差分) →
//! 使われうる prelude のアイテムを後ろに付ける。編集をまたいで覚えるもの (FrontMemo) を渡すと、変わっていない所を作り直さない

use crate::lexer::{self, ItemKind, SrcItem};
use crate::prof::{Timer, now_ns};

pub type R<T> = Result<T, String>;

/// ユーザのソースの後ろに、使われうる prelude のアイテムを付ける (driver::with_prelude と同じ)。
/// 返り値は (全体のソース, アイテム, ユーザのアイテムの数)
pub fn with_prelude(src: &str) -> R<(String, Vec<SrcItem>, usize)> {
    with_prelude_in(src, &[], &mut vec![], None, &mut Default::default()).map(|(s, i, u, _)| (s, i, u))
}

/// 前処理の、編集をまたいで覚えておくもの (LSP の didChange で変わっていない所を作り直さない)
#[derive(Default)]
pub struct FrontMemo {
    pub sel: crate::prelude::SelectCache,
    /// 前回の split の入力と結果
    pub split: Option<(String, Vec<SrcItem>)>,
    /// 前処理のチャンクごとの結果
    pub prep: crate::prep::PrepMemo,
}

fn clone_item(it: &SrcItem) -> SrcItem {
    SrcItem { start: it.start, end: it.end, line: it.line, hash: it.hash, idents: it.idents.clone(), kind: it.kind }
}

fn count_nl(b: &[u8]) -> usize {
    b.iter().filter(|&&c| c == b'\n').count()
}

/// lexer::split を、前回の入力と結果から差分で作る: 前後の変わっていない部分のアイテムはそのまま (後ろは位置をずらす) 使い、
/// 間だけを split し直す。区切りは、空白の後から始まるトップレベルのアイテム (impl / trait のブロックは丸ごと) の先頭。
/// 間の終わりがコメント・文字列の途中になりうるときや、split が失敗するときは全体を split する
#[doc(hidden)]
pub fn split_inc(src: &str, memo: &mut Option<(String, Vec<SrcItem>)>) -> R<Vec<SrcItem>> {
    let r = match memo.as_ref() {
        Some((old, items)) if std::env::var_os("CRUSTC_NO_SPLIT_INC").is_none() => split_diff(old, items, src),
        _ => None,
    };
    let items = match r {
        Some(v) => v,
        None => lexer::split(src)?,
    };
    if std::env::var_os("CRUSTC_SPLIT_VERIFY").is_some() {
        let full = lexer::split(src)?;
        let same = full.len() == items.len()
            && full.iter().zip(&items).all(|(a, b)| a.start == b.start && a.end == b.end && a.line == b.line && a.hash == b.hash && a.idents == b.idents && a.kind == b.kind);
        if !same {
            return Err("internal: incremental split differs from the full split".into());
        }
    }
    *memo = Some((src.to_string(), items.iter().map(clone_item).collect()));
    Ok(items)
}

fn split_diff(old: &str, items: &[SrcItem], new: &str) -> Option<Vec<SrcItem>> {
    let (p, n) = (old.as_bytes(), new.as_bytes());
    if p == n {
        return Some(items.iter().map(clone_item).collect());
    }
    let lim = p.len().min(n.len());
    let mut pre = 0;
    while pre < lim && p[pre] == n[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < lim - pre && p[p.len() - 1 - suf] == n[n.len() - 1 - suf] {
        suf += 1;
    }
    // 区切り: (アイテムの番号, 位置)
    let mut groups: Vec<(usize, usize)> = vec![];
    for (k, it) in items.iter().enumerate() {
        let top = !matches!(it.kind, ItemKind::ImplMember(_) | ItemKind::TraitMember(_));
        if top && (it.start == 0 || p[it.start - 1].is_ascii_whitespace()) {
            groups.push((k, it.start));
        }
    }
    // 前: 区切りの範囲 (次の区切りまで) が変わった所より前にあるもの
    let mut j = 0;
    while j + 1 < groups.len() && groups[j + 1].1 <= pre {
        j += 1;
    }
    // (groups[j] が変わった所を含む。先頭のアイテムより前が変わったら最初から)
    let (keep_n, a) = if groups.is_empty() || groups[j].1 > pre { (0, 0) } else { (groups[j].0, groups[j].1) };
    // 後ろ: 変わった所より後から始まる最初の区切り
    let chg_end = p.len() - suf;
    let m = groups.iter().position(|g| g.1 > chg_end && g.1 > a);
    let (tail_k, b_old) = match m {
        Some(m) => (groups[m].0, groups[m].1),
        None => (items.len(), p.len()),
    };
    let b_new = (b_old + n.len()).checked_sub(p.len())?;
    if b_new < a {
        return None;
    }
    let mid = &new[a..b_new];
    let mut mids = lexer::split(mid).ok()?;
    // (間の終わりの、アイテムの後ろ: コメントや文字列が開いたままかもしれないものがあれば使わない)
    let last_end = mids.last().map(|x| x.end).unwrap_or(0);
    let gap = &mid.as_bytes()[last_end..];
    if gap.windows(2).any(|w| w == b"/*") || gap.iter().any(|&c| c == b'"' || c == b'\'' || c == b'#') {
        return None;
    }
    let mut out: Vec<SrcItem> = Vec::with_capacity(keep_n + mids.len() + items.len() - tail_k);
    out.extend(items[..keep_n].iter().map(clone_item));
    let lines_a = count_nl(&n[..a]);
    for it in &mut mids {
        it.start += a;
        it.end += a;
        it.line += lines_a;
        for x in &mut it.idents {
            x.0 += a as u32;
            x.1 += a as u32;
        }
        it.kind = match it.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember(h + keep_n as u32),
            ItemKind::TraitMember(h) => ItemKind::TraitMember(h + keep_n as u32),
            k => k,
        };
    }
    out.extend(mids);
    let dk = out.len() as i64 - tail_k as i64;
    let dpos = n.len() as i64 - p.len() as i64;
    let dline = count_nl(&n[..b_new]) as i64 - count_nl(&p[..b_old]) as i64;
    for it in &items[tail_k..] {
        let mut x = clone_item(it);
        x.start = (x.start as i64 + dpos) as usize;
        x.end = (x.end as i64 + dpos) as usize;
        x.line = (x.line as i64 + dline) as usize;
        for y in &mut x.idents {
            y.0 = (y.0 as i64 + dpos) as u32;
            y.1 = (y.1 as i64 + dpos) as u32;
        }
        x.kind = match x.kind {
            ItemKind::ImplMember(h) => ItemKind::ImplMember((h as i64 + dk) as u32),
            ItemKind::TraitMember(h) => ItemKind::TraitMember((h as i64 + dk) as u32),
            k => k,
        };
        out.push(x);
    }
    Some(out)
}

/// crates: ソースの中のクレートのモジュールの名前 (lexer::rename_module_collisions_in)
/// fix: モジュールの名前を変えて桁がずれた所 (lexer::rename_module_collisions_fix)
/// cfg: マクロを展開したものの中の `#[cfg(..)]` を評価する設定
/// 返り値の 4 つ目は prelude の先頭の行 (prelude の中の行 = 結合したソースの行 - これ)
pub fn with_prelude_in(src: &str, crates: &[String], fix: &mut Vec<(u32, u32, i32)>, cfg: Option<&crate::cfg::CfgSet>, cache: &mut FrontMemo) -> R<(String, Vec<SrcItem>, usize, usize)> {
    let mut tm = Timer { on: std::env::var_os("CRUSTC_PROF").is_some(), last: now_ns(), start: now_ns() };
    let pre = crate::prep::prep(src, crates, fix, cfg, &mut cache.prep)?;
    let src: &str = &pre;
    tm.lap("prep", String::new());
    let mut items = split_inc(src, &mut cache.split)?;
    let user = items.len();
    tm.lap("split", String::new());
    let extra = crate::prelude::select_with(src, &items, &mut cache.sel);
    tm.lap("prelude", String::new());
    if extra.is_empty() {
        return Ok((src.to_string(), items, user, 0));
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
    Ok((full, items, user, lines))
}
