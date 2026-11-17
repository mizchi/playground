use super::scale;

/// A word.
pub struct Word {
    pub text: String,
}

impl Word {
    pub fn new(s: &str) -> Word {
        Word { text: s.to_string() }
    }
    pub fn len(&self) -> usize {
        self.text.len()
    }
    pub fn upper(&self) -> String {
        let mut out = String::new();
        out.push_str(self.text.as_str());
        out
    }
}

pub fn shout_len(s: &str) -> usize {
    let w = Word::new(s);
    let f = scale(1.0);
    w.upper().len() + f as usize
}
