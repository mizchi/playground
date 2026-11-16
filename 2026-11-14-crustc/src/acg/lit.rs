//! リテラルのトークン (自前の構文木では文字列のまま) を syn::Lit と同じ形で見るためのもの。

#[derive(Clone, Debug)]
pub struct LitInt {
    digits: String,
    suffix: String,
}
impl LitInt {
    pub fn suffix(&self) -> &str {
        &self.suffix
    }
    pub fn base10_digits(&self) -> &str {
        &self.digits
    }
    pub fn value(&self) -> Option<u128> {
        self.digits.parse().ok()
    }
}

#[derive(Clone, Debug)]
pub struct LitFloat {
    digits: String,
    suffix: String,
}
impl LitFloat {
    pub fn suffix(&self) -> &str {
        &self.suffix
    }
    pub fn base10_digits(&self) -> &str {
        &self.digits
    }
    pub fn value(&self) -> f64 {
        self.digits.parse().unwrap_or(0.0)
    }
}

#[derive(Clone, Debug)]
pub enum Lit {
    Int(LitInt),
    Float(LitFloat),
    Bool(bool),
    Byte(u8),
    Char(char),
    /// 文字列 (中身)
    Str(String),
    ByteStr(Vec<u8>),
    CStr(Vec<u8>),
}

/// リテラルのトークンのテキストを分類する
pub fn lit(t: &str) -> Option<Lit> {
    if t == "true" {
        return Some(Lit::Bool(true));
    }
    if t == "false" {
        return Some(Lit::Bool(false));
    }
    if let Some(r) = t.strip_prefix("b'") {
        let inner = r.strip_suffix('\'')?;
        return crate::program::char_lit(inner).map(|c| Lit::Byte(c as u32 as u8));
    }
    if let Some(r) = t.strip_prefix('\'') {
        let inner = r.strip_suffix('\'')?;
        return crate::program::char_lit(inner).map(Lit::Char);
    }
    if t.starts_with("c\"") || t.starts_with("cr") {
        let (_, b) = crate::ast_consteval::str_lit(&t[1..])?;
        return Some(Lit::CStr(b));
    }
    if t.starts_with('"') || t.starts_with("r\"") || t.starts_with("r#") || t.starts_with("b\"") || t.starts_with("br") {
        let (byte, b) = crate::ast_consteval::str_lit(t)?;
        return Some(if byte { Lit::ByteStr(b) } else { Lit::Str(String::from_utf8(b).ok()?) });
    }
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let s: String = t.chars().filter(|&c| c != '_').collect();
    let (radix, body) = if let Some(h) = s.strip_prefix("0x") {
        (16, h)
    } else if let Some(b) = s.strip_prefix("0b") {
        (2, b)
    } else if let Some(o) = s.strip_prefix("0o") {
        (8, o)
    } else {
        (10, s.as_str())
    };
    let digits: String = body.chars().take_while(|c| c.is_digit(radix)).collect();
    let rest = &body[digits.len()..];
    // 浮動小数点 (`1.5` / `1e3` / `2.` / `1f64`)
    if radix == 10 && (rest.starts_with('.') || rest.starts_with('e') || rest.starts_with('E') || rest == "f32" || rest == "f64") {
        let mut n = digits.len();
        let b = body.as_bytes();
        if n < b.len() && b[n] == b'.' {
            n += 1;
            while n < b.len() && b[n].is_ascii_digit() {
                n += 1;
            }
        }
        if n < b.len() && (b[n] == b'e' || b[n] == b'E') {
            n += 1;
            if n < b.len() && (b[n] == b'+' || b[n] == b'-') {
                n += 1;
            }
            while n < b.len() && b[n].is_ascii_digit() {
                n += 1;
            }
        }
        let suffix = body[n..].to_string();
        if (rest == "f32" || rest == "f64") && suffix == rest {
            // (`1f64` は syn では整数リテラル + 接尾辞 f64)
            return Some(Lit::Int(LitInt { digits, suffix }));
        }
        return Some(Lit::Float(LitFloat { digits: body[..n].to_string(), suffix }));
    }
    let mut v: u128 = 0;
    for c in digits.chars() {
        v = v.checked_mul(radix as u128)?.checked_add(c.to_digit(radix)? as u128)?;
    }
    Some(Lit::Int(LitInt { digits: v.to_string(), suffix: rest.to_string() }))
}
