// std::borrow::Cow (スライスと str)、copy_from_slice / clone_from_slice、u64::from_le_bytes とスライス
use std::borrow::Cow;

fn bytes(d: &[u8], double: bool) -> Cow<'_, [u8]> {
    if !double {
        return Cow::Borrowed(d);
    }
    Cow::Owned(d.iter().map(|&b| b * 2).collect())
}

fn le(d: &[u8], o: usize, n: usize) -> Option<u64> {
    let b = d.get(o..o + n)?;
    let mut v = [0u8; 8];
    v[..n].copy_from_slice(b);
    Some(u64::from_le_bytes(v))
}

fn main() {
    let d = [1u8, 2, 3, 4];
    let a = bytes(&d, false);
    let b = bytes(&d, true);
    println!("{:?} {:?} {} {}", a.to_vec(), b.to_vec(), a.len(), matches!(b, Cow::Owned(_)));
    println!("{:?}", b.into_owned());
    println!("{:?} {:?} {:?}", le(&d, 0, 2), le(&d, 1, 3), le(&d, 3, 4));
    let mut w = vec![String::new(); 2];
    w.clone_from_slice(&["x".to_string(), "y".to_string()]);
    let mut x = [0i32; 5];
    x[1..4].copy_from_slice(&[7, 8, 9]);
    println!("{:?} {:?}", w, x);
    let s = String::from_utf8_lossy(b"ok\xff").into_owned();
    println!("{s}");
}
