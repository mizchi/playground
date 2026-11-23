//! Todo アプリを WIT のコンポーネントにしたもの (src/apps/todo.ts と同じ振る舞い)。
//! import は host.emit と storage (能力 storage)。std が持ち込む WASI の import には、ホストが拒否する実装を渡す (ファイルは読めない)。
wit_bindgen::generate!({ world: "gui-app-storage", path: "../wit" });

use exports::gui::app::app::Guest;
use gui::app::host::emit;
use gui::app::storage;
use gui::app::types::{ActionName, ActionRequest, Node, Role, TreeUpdate};
use std::cell::RefCell;

struct Item {
    id: String,
    text: String,
    done: bool,
}

struct State {
    items: Vec<Item>,
    draft: String,
    focus: Option<String>,
    sent: Vec<String>,
    seq: u32,
    status: String,
    /// この窓で保存領域を読めたか (読めていなければ保存しない)
    loaded: bool,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State {
        items: vec![
            Item { id: "i1".into(), text: "牛乳を買う".into(), done: false },
            Item { id: "i2".into(), text: "請求書を送る".into(), done: true },
            Item { id: "i3".into(), text: "PR をレビューする".into(), done: false },
        ],
        draft: String::new(),
        focus: None,
        sent: vec![],
        seq: 4,
        status: String::new(),
        loaded: false,
    });
}

fn node(id: &str, role: Role, label: &str) -> Node {
    Node { id: id.into(), role, label: Some(label.into()), value: None, checked: None, disabled: false, sensitive: false, actions: vec![], children: vec![] }
}

fn build(root: bool) -> TreeUpdate {
    S.with(|s| {
        let s = &mut *s.borrow_mut();
        let left = s.items.iter().filter(|i| !i.done).count();
        let mut nodes = vec![
            Node { children: vec!["title".into(), "list".into(), "entry".into(), "probe".into(), "status".into()], ..node("root", Role::XWindow, "Todo (wasm)") },
            node("title", Role::Heading, "やること"),
            Node { children: s.items.iter().map(|i| i.id.clone()).collect(), ..node("list", Role::List, "タスク") },
        ];
        for i in &s.items {
            nodes.push(Node { checked: Some(i.done), actions: vec![ActionName::Click, ActionName::Focus], ..node(&i.id, Role::Checkbox, &i.text) });
        }
        nodes.push(Node { children: vec!["input".into(), "add".into()], ..node("entry", Role::Group, "追加") });
        nodes.push(Node { value: Some(s.draft.clone()), actions: vec![ActionName::Focus, ActionName::SetValue, ActionName::ReplaceSelectedText], ..node("input", Role::Textbox, "新しいタスク") });
        nodes.push(Node { actions: vec![ActionName::Click, ActionName::Focus], ..node("add", Role::Button, "追加") });
        nodes.push(Node { actions: vec![ActionName::Click, ActionName::Focus], ..node("probe", Role::Button, "ファイルを読んでみる") });
        let status = if s.status.is_empty() { format!("残り {left} 件") } else { s.status.clone() };
        nodes.push(node("status", Role::Paragraph, &status));
        let ids: Vec<String> = nodes.iter().map(|n| n.id.clone()).collect();
        let remove = s.sent.iter().filter(|id| !ids.contains(id)).cloned().collect();
        s.sent = ids;
        TreeUpdate { root: if root { Some("root".into()) } else { None }, focus: s.focus.clone(), nodes, remove }
    })
}

/// 文字列の \ \t \n を逃がす (保存の形 "id\tdone\ttext" の区切りと混ざらないように。元に戻せる)
fn escape(t: &str) -> String {
    t.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn unescape(t: &str) -> String {
    let mut out = String::new();
    let mut it = t.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// 項目を 1 行ずつ "id\tdone\ttext" で保存する (serde を使わない最小の形)。
/// この窓で保存領域を読めていなければ書かない (既定の項目で、保存済みの項目を上書きしない)
fn save() {
    let (loaded, data) = S.with(|s| {
        let s = s.borrow();
        (s.loaded, s.items.iter().map(|i| format!("{}\t{}\t{}", i.id, if i.done { 1 } else { 0 }, escape(&i.text))).collect::<Vec<_>>().join("\n"))
    });
    if !loaded {
        // 起動のときに読めなかった: 書く前にもう一度読む。読めたら保存済みの項目に戻す (既定の項目への操作は捨てる)
        load();
        let status = if S.with(|s| s.borrow().loaded) { "保存済みの項目を読み直した (今の操作は反映していない)" } else { "保存領域を読めないので、保存しない" };
        S.with(|s| s.borrow_mut().status = status.into());
        return;
    }
    if !storage::set("items", &data) {
        S.with(|s| s.borrow_mut().status = "保存できなかった".into());
    }
}

/// 保存領域から読む。読めなければ (err) loaded を false のままにする。キーが無いだけなら既定の項目で始める
fn load() {
    let data = match storage::get("items") {
        Err(_) => return,
        Ok(None) => {
            S.with(|s| s.borrow_mut().loaded = true);
            return;
        }
        Ok(Some(d)) => d,
    };
    let items: Vec<Item> = data
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some(Item { id: p.next()?.into(), done: p.next()? == "1", text: unescape(p.next()?) })
        })
        .collect();
    S.with(|s| {
        let s = &mut *s.borrow_mut();
        s.seq = items.iter().filter_map(|i| i.id.strip_prefix('i')?.parse::<u32>().ok()).max().unwrap_or(0) + 1;
        s.items = items;
        s.loaded = true;
    });
}

struct Component;

impl Guest for Component {
    fn start() {
        load();
        // 借用を外してから host を呼ぶ (host が同期で呼び返しても困らないように)
        let u = build(true);
        emit(&u);
    }

    fn on_action(req: ActionRequest) {
        let changed = matches!((req.target.as_str(), req.action), ("add", ActionName::Click)) || (req.action == ActionName::Click && req.target.starts_with('i'));
        S.with(|s| {
            let s = &mut *s.borrow_mut();
            match (req.target.as_str(), req.action) {
                (_, ActionName::Focus) => s.focus = Some(req.target.clone()),
                ("input", ActionName::SetValue) => s.draft = req.data.clone().unwrap_or_default(),
                ("input", ActionName::ReplaceSelectedText) => s.draft.push_str(&req.data.clone().unwrap_or_default()),
                ("add", ActionName::Click) => {
                    let text = s.draft.trim().to_string();
                    if !text.is_empty() {
                        let id = format!("i{}", s.seq);
                        s.seq += 1;
                        s.items.push(Item { id, text, done: false });
                    }
                    s.draft.clear();
                }
                ("probe", ActionName::Click) => {
                    // 能力の確認: ファイルを読もうとする。ホストが WASI のファイルを渡していなければ失敗する
                    s.status = match std::fs::read_to_string("/etc/hostname") {
                        Ok(t) => format!("読めた: {}", t.trim()),
                        Err(e) => format!("ファイルは読めない ({})", e.kind()),
                    };
                }
                (id, ActionName::Click) => {
                    if let Some(i) = s.items.iter_mut().find(|i| i.id == id) {
                        i.done = !i.done;
                    }
                }
                _ => {}
            }
        });
        // 借用を外してから保存する (host の呼び出しの中で状態を借りない)
        if changed {
            save();
        }
        let u = build(false);
        emit(&u);
    }
}

export!(Component);
