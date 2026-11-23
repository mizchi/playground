//! 応答しないコンポーネントの見本 (検証用)。「固まる」を押すと on-action から戻らない。
//! ホストはコンポーネントを Worker で動かし、期限までに応答しなければ止める。ほかの窓は固まらない。
wit_bindgen::generate!({ world: "gui-app", path: "../wit" });

use exports::gui::app::app::Guest;
use gui::app::host::emit;
use gui::app::types::{ActionName, ActionRequest, Node, Role, TreeUpdate};
use std::cell::Cell;

thread_local! {
    static COUNT: Cell<u32> = const { Cell::new(0) };
}

fn node(id: &str, role: Role, label: &str) -> Node {
    Node { id: id.into(), role, label: Some(label.into()), value: None, checked: None, disabled: false, sensitive: false, actions: vec![], children: vec![] }
}

fn publish(root: bool) {
    let n = COUNT.with(|c| c.get());
    let nodes = vec![
        Node { children: vec!["title".into(), "count".into(), "spin".into(), "status".into()], ..node("root", Role::XWindow, "Spin (wasm)") },
        node("title", Role::Heading, "応答しないコンポーネント"),
        Node { actions: vec![ActionName::Click, ActionName::Focus], ..node("count", Role::Button, &format!("数える ({n})")) },
        Node { actions: vec![ActionName::Click, ActionName::Focus], ..node("spin", Role::Button, "固まる") },
        node("status", Role::Paragraph, "「固まる」を押すと、on-action から戻らない"),
    ];
    emit(&TreeUpdate { root: if root { Some("root".into()) } else { None }, focus: None, nodes, remove: vec![] });
}

struct Component;

impl Guest for Component {
    fn start() {
        publish(true);
    }

    fn on_action(req: ActionRequest) {
        match (req.target.as_str(), req.action) {
            ("count", ActionName::Click) => COUNT.with(|c| c.set(c.get() + 1)),
            ("spin", ActionName::Click) => {
                // 戻らない (Rust では無限ループは未定義動作ではない)
                #[allow(clippy::empty_loop)]
                loop {
                    std::hint::spin_loop();
                }
            }
            _ => {}
        }
        publish(false);
    }
}

export!(Component);
