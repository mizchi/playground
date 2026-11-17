use fixture::{Counter, Describe};

fn helper() -> Counter {
    Counter::new("it")
}

#[test]
fn integration() {
    let c = helper();
    let d = c.describe();
    assert!(d.len() > 0);
    assert_eq!(c.twice().len(), 4);
}
