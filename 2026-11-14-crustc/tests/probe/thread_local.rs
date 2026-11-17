// thread_local! (遅延初期化・RefCell / Cell・モジュールの中)
use std::cell::{Cell, RefCell};

thread_local! {
    /// 作っている途中のもの
    static STACK: RefCell<Vec<(u32, String)>> = const { RefCell::new(Vec::new()) };
    static COUNT: Cell<u64> = Cell::new(10);
}

mod inner {
    use std::cell::RefCell;
    thread_local! {
        pub static NAMES: RefCell<Vec<String>> = RefCell::new(vec!["x".to_string()]);
    }
    pub fn add(s: &str) -> usize {
        NAMES.with(|n| {
            n.borrow_mut().push(s.to_string());
            n.borrow().len()
        })
    }
}

fn push(i: u32) -> usize {
    STACK.with(|s| {
        s.borrow_mut().push((i, format!("v{i}")));
        s.borrow().len()
    })
}

fn main() {
    for i in 0..5 {
        push(i * 3);
        COUNT.with(|c| c.set(c.get() + i as u64));
    }
    let last = STACK.with(|s| s.borrow().last().cloned());
    let total = COUNT.with(|c| c.get());
    println!("{:?} {} {}", last, total, push(99));
    println!("{} {}", inner::add("y"), inner::add("z"));
    inner::NAMES.with(|n| println!("{:?}", n.borrow()));
}
