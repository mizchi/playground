// Drop のサンプル: スコープ・ムーブ・代入・break / return・match・Option・Box<T> / Box<dyn Trait>・
// 再帰的なリスト・クロージャのキャプチャ・ジェネリックな Drop。drop の順序を rustc と比べる
unsafe extern "C" {
    fn putchar(c: i32) -> i32;
}
fn put(c: u8) {
    unsafe {
        putchar(c as i32);
    }
}
fn num(n: i64) {
    if n < 0 {
        put(b'-');
        num(-n);
        return;
    }
    if n >= 10 {
        num(n / 10);
    }
    put(b'0' + (n % 10) as u8);
}
fn line(n: i64) {
    num(n);
    put(b'\n');
}

struct Noisy(u8);

impl Drop for Noisy {
    fn drop(&mut self) {
        put(b'~');
        put(self.0);
        put(b' ');
    }
}

fn make(c: u8) -> Noisy {
    Noisy(c)
}

fn consume(n: Noisy) {
    put(b'c');
    put(n.0);
    put(b' ');
}

fn early(flag: bool) -> u8 {
    let _a = Noisy(b'p');
    if flag {
        let _b = Noisy(b'q');
        return 1;
    }
    let _c = Noisy(b'r');
    2
}

struct Pair {
    a: Noisy,
    b: Box<Noisy>,
}

struct Guard<T> {
    v: T,
    tag: u8,
}

impl<T> Drop for Guard<T> {
    fn drop(&mut self) {
        put(b'G');
        put(self.tag);
        put(b' ');
    }
}

trait Animal {
    fn name(&self) -> u8;
}

struct Dog(Noisy);

impl Animal for Dog {
    fn name(&self) -> u8 {
        b'D'
    }
}

struct Node {
    v: i64,
    next: Option<Box<Node>>,
}

fn push(list: Option<Box<Node>>, v: i64) -> Option<Box<Node>> {
    Some(Box::new(Node { v, next: list }))
}

fn total(mut cur: &Option<Box<Node>>) -> i64 {
    let mut s = 0;
    while let Some(n) = cur {
        s += n.v;
        cur = &n.next;
    }
    s
}

struct Stack {
    top: Option<Box<Node>>,
    len: i64,
}

impl Stack {
    fn push(&mut self, v: i64) {
        let old = self.top.take();
        self.top = Some(Box::new(Node { v, next: old }));
        self.len += 1;
    }
    fn pop(&mut self) -> Option<i64> {
        match self.top.take() {
            Some(n) => {
                self.top = n.next;
                self.len -= 1;
                Some(n.v)
            }
            None => None,
        }
    }
}

fn main() {
    // スコープの終わりで逆順に
    let _a = Noisy(b'a');
    {
        let b = Noisy(b'b');
        put(b.0);
        put(b' ');
    }
    // ムーブしたものは drop しない
    let c = Noisy(b'c');
    consume(c);
    let d = Noisy(b'd');
    let e = d;
    put(e.0);
    put(b'\n');
    // 捨てた値・`let _ =` はその場で
    make(b't');
    let _ = make(b'u');
    // 代入は古い値を drop する
    let mut f = Noisy(b'f');
    f = Noisy(b'g');
    put(f.0);
    put(b'\n');
    // break / return
    for i in 0..3u8 {
        let x = Noisy(b'0' + i);
        if i == 1 {
            break;
        }
        put(x.0);
    }
    put(b'\n');
    put(b'0' + early(true));
    put(b'0' + early(false));
    put(b'\n');
    // match / if let はパターンがムーブする
    let o = Some(Noisy(b'o'));
    if let Some(n) = o {
        put(n.0);
    }
    let o2 = Some(Noisy(b'O'));
    match &o2 {
        Some(n) => put(n.0),
        None => {}
    }
    put(b'\n');
    // struct のフィールド (Box も)
    let p = Pair {
        a: Noisy(b'1'),
        b: Box::new(Noisy(b'2')),
    };
    put(p.b.0);
    put(b'\n');
    drop(p);
    put(b'\n');
    // ジェネリックな Drop と、中身の drop の順
    let g = Guard {
        v: Noisy(b'v'),
        tag: b'x',
    };
    put(g.v.0);
    put(b'\n');
    // Box<dyn Trait>: 中身 (Dog → Noisy) まで drop する
    let animals: [Box<dyn Animal>; 2] = [Box::new(Dog(Noisy(b'w'))), Box::new(Dog(Noisy(b'z')))];
    for an in &animals {
        put(an.name());
    }
    put(b'\n');
    // クロージャの move キャプチャはクロージャと一緒に drop する
    let cap = Noisy(b'k');
    let show = move || put(cap.0);
    show();
    show();
    put(b'\n');
    // 再帰的なリスト (全部 free される)
    let mut l = None;
    for i in 1..=4 {
        l = push(l, i * 10);
    }
    line(total(&l));
    let mut st = Stack { top: None, len: 0 };
    for i in 1..=5 {
        st.push(i);
    }
    let mut popped = 0;
    while let Some(v) = st.pop() {
        popped = popped * 10 + v;
        if st.len == 2 {
            break;
        }
    }
    line(popped);
    line(st.len);
}
