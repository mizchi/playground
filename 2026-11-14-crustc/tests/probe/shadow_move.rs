// match 対象と同じ名前の束縛 (`let nn = ..; if let Some(nn) = nn { v.push(nn) }`) で値をムーブする:
// 外側の変数をムーブ済みにする (二重に drop しない)。while let / match も
fn find(names: &[(u32, Vec<u8>)], k: u32) -> Option<Vec<u8>> {
    names.iter().find(|n| n.0 == k).map(|n| n.1.clone())
}

fn main() {
    let names = vec![(1, b"one".to_vec()), (2, b"two".to_vec())];
    let mut out: Vec<Vec<u8>> = vec![];
    for k in 0..4 {
        let nn = find(&names, k).or_else(|| find(&names, k + 1));
        if let Some(nn) = nn {
            out.push(nn);
        }
    }
    let s = Some(String::from("moved"));
    match s {
        Some(s) => out.push(s.into_bytes()),
        None => {}
    }
    let mut stack = vec![String::from("a"), String::from("b")];
    let mut top = stack.pop();
    while let Some(top2) = top {
        out.push(top2.into_bytes());
        top = stack.pop();
    }
    let v: Vec<String> = out.iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
    println!("{:?}", v);
}
