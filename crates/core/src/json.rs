//! Small bounded JSON codec for LSP and local configuration; no external crates.
use std::collections::BTreeMap;
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}
impl Value {
    pub fn object(items: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Self {
        Self::Object(items.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    pub fn get(&self, key: &str) -> Option<&Self> {
        if let Self::Object(m) = self {
            m.get(key)
        } else {
            None
        }
    }
    pub fn str(&self) -> Option<&str> {
        if let Self::String(s) = self {
            Some(s)
        } else {
            None
        }
    }
    pub fn array(&self) -> Option<&[Self]> {
        if let Self::Array(a) = self {
            Some(a)
        } else {
            None
        }
    }
    pub fn integer(&self) -> Option<u64> {
        if let Self::Number(n) = self
            && n.is_finite()
            && *n >= 0.0
            && n.fract() == 0.0
            && *n <= 9_007_199_254_740_991.0
        {
            return Some(*n as u64);
        }
        None
    }
    pub fn encode(&self) -> String {
        match self {
            Self::Null => "null".into(),
            Self::Bool(b) => b.to_string(),
            Self::Number(n) => {
                if n.is_finite() {
                    n.to_string()
                } else {
                    "null".into()
                }
            }
            Self::String(s) => {
                let mut out = String::from("\"");
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
                        c => out.push(c),
                    }
                }
                out.push('"');
                out
            }
            Self::Array(a) => format!(
                "[{}]",
                a.iter().map(Self::encode).collect::<Vec<_>>().join(",")
            ),
            Self::Object(m) => format!(
                "{{{}}}",
                m.iter()
                    .map(|(k, v)| format!("{}:{}", Self::String(k.clone()).encode(), v.encode()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Self::String(v.into())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}
impl From<u32> for Value {
    fn from(v: u32) -> Self {
        Self::Number(v as f64)
    }
}
struct Parser<'a> {
    s: &'a str,
    pos: usize,
    nodes: usize,
}
impl Parser<'_> {
    fn ws(&mut self) {
        while self
            .s
            .as_bytes()
            .get(self.pos)
            .is_some_and(|c| matches!(c, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.pos += 1;
        }
    }
    fn byte(&mut self, b: u8) -> bool {
        self.ws();
        if self.s.as_bytes().get(self.pos) == Some(&b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn string(&mut self) -> Result<String, String> {
        if !self.byte(b'"') {
            return Err("Expected JSON string".into());
        }
        let mut out = String::new();
        loop {
            let c = self.s[self.pos..]
                .chars()
                .next()
                .ok_or("Unterminated JSON string")?;
            self.pos += c.len_utf8();
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let e = *self
                        .s
                        .as_bytes()
                        .get(self.pos)
                        .ok_or("Incomplete JSON escape")?;
                    self.pos += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0c'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let a = self.hex()?;
                            let code = if (0xd800..=0xdbff).contains(&a) {
                                if !self.s[self.pos..].starts_with("\\u") {
                                    return Err("Missing low surrogate".into());
                                }
                                self.pos += 2;
                                let b = self.hex()?;
                                if !(0xdc00..=0xdfff).contains(&b) {
                                    return Err("Invalid low surrogate".into());
                                }
                                0x10000 + ((a - 0xd800) << 10) + (b - 0xdc00)
                            } else {
                                a
                            };
                            out.push(char::from_u32(code).ok_or("Invalid Unicode escape")?);
                        }
                        _ => return Err("Invalid JSON escape".into()),
                    }
                }
                c if c < ' ' => return Err("Unescaped JSON control character".into()),
                c => out.push(c),
            }
        }
    }
    fn hex(&mut self) -> Result<u32, String> {
        let end = self.pos.checked_add(4).ok_or("Escape overflow")?;
        let value = u32::from_str_radix(
            self.s
                .get(self.pos..end)
                .ok_or("Incomplete Unicode escape")?,
            16,
        )
        .map_err(|_| "Invalid Unicode escape")?;
        self.pos = end;
        Ok(value)
    }
    fn value(&mut self, depth: usize) -> Result<Value, String> {
        self.ws();
        self.nodes += 1;
        if depth > 64 || self.nodes > 100_000 {
            return Err("JSON structure budget exceeded".into());
        }
        let b = *self
            .s
            .as_bytes()
            .get(self.pos)
            .ok_or("Missing JSON value")?;
        match b {
            b'"' => Ok(Value::String(self.string()?)),
            b'[' => {
                self.pos += 1;
                let mut a = Vec::new();
                if self.byte(b']') {
                    return Ok(Value::Array(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    if self.byte(b']') {
                        break;
                    }
                    if !self.byte(b',') {
                        return Err("Missing array comma".into());
                    }
                }
                Ok(Value::Array(a))
            }
            b'{' => {
                self.pos += 1;
                let mut m = BTreeMap::new();
                if self.byte(b'}') {
                    return Ok(Value::Object(m));
                }
                loop {
                    let key = self.string()?;
                    if !self.byte(b':') {
                        return Err("Missing object colon".into());
                    }
                    let value = self.value(depth + 1)?;
                    if m.insert(key, value).is_some() {
                        return Err("Duplicate JSON key".into());
                    }
                    if self.byte(b'}') {
                        break;
                    }
                    if !self.byte(b',') {
                        return Err("Missing object comma".into());
                    }
                }
                Ok(Value::Object(m))
            }
            b't' | b'f' | b'n' => {
                let (lit, v) = match b {
                    b't' => ("true", Value::Bool(true)),
                    b'f' => ("false", Value::Bool(false)),
                    _ => ("null", Value::Null),
                };
                if !self.s[self.pos..].starts_with(lit) {
                    return Err("Invalid JSON literal".into());
                }
                self.pos += lit.len();
                Ok(v)
            }
            b'-' | b'0'..=b'9' => {
                let start = self.pos;
                if b == b'-' {
                    self.pos += 1;
                }
                if self.s.as_bytes().get(self.pos) == Some(&b'0') {
                    self.pos += 1;
                } else {
                    let before = self.pos;
                    while self
                        .s
                        .as_bytes()
                        .get(self.pos)
                        .is_some_and(u8::is_ascii_digit)
                    {
                        self.pos += 1;
                    }
                    if before == self.pos {
                        return Err("Invalid number".into());
                    }
                }
                if self.s.as_bytes().get(self.pos) == Some(&b'.') {
                    self.pos += 1;
                    let n = self.pos;
                    while self
                        .s
                        .as_bytes()
                        .get(self.pos)
                        .is_some_and(u8::is_ascii_digit)
                    {
                        self.pos += 1;
                    }
                    if n == self.pos {
                        return Err("Missing fraction".into());
                    }
                }
                if self
                    .s
                    .as_bytes()
                    .get(self.pos)
                    .is_some_and(|b| matches!(b, b'e' | b'E'))
                {
                    self.pos += 1;
                    if self
                        .s
                        .as_bytes()
                        .get(self.pos)
                        .is_some_and(|b| matches!(b, b'+' | b'-'))
                    {
                        self.pos += 1;
                    }
                    let n = self.pos;
                    while self
                        .s
                        .as_bytes()
                        .get(self.pos)
                        .is_some_and(u8::is_ascii_digit)
                    {
                        self.pos += 1;
                    }
                    if n == self.pos {
                        return Err("Missing exponent".into());
                    }
                }
                let n = self.s[start..self.pos]
                    .parse::<f64>()
                    .map_err(|_| "Invalid JSON number")?;
                if !n.is_finite() {
                    return Err("JSON number overflow".into());
                }
                Ok(Value::Number(n))
            }
            _ => Err("Invalid JSON value".into()),
        }
    }
}
pub fn parse(s: &str) -> Result<Value, String> {
    if s.len() > 16 * 1024 * 1024 {
        return Err("JSON payload budget exceeded".into());
    }
    let mut p = Parser {
        s,
        pos: 0,
        nodes: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.pos != s.len() {
        return Err("Trailing JSON data".into());
    }
    Ok(v)
}
pub fn frame(value: &Value) -> Vec<u8> {
    let body = value.encode();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}
pub fn read_frame(input: &mut impl std::io::Read) -> Result<Value, String> {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        input.read_exact(&mut byte).map_err(|e| e.to_string())?;
        header.push(byte[0]);
        if header.len() > 16384 {
            return Err("LSP header budget exceeded".into());
        }
    }
    let text = std::str::from_utf8(&header).map_err(|_| "Non-ASCII LSP header")?;
    let mut length = None;
    for line in text.split("\r\n") {
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("Content-Length")
        {
            if length.is_some() {
                return Err("Duplicate Content-Length".into());
            }
            let n = value
                .trim()
                .parse::<usize>()
                .map_err(|_| "Invalid Content-Length")?;
            if n > 16 * 1024 * 1024 {
                return Err("LSP body budget exceeded".into());
            }
            length = Some(n);
        }
    }
    let mut body = vec![0; length.ok_or("Missing Content-Length")?];
    input.read_exact(&mut body).map_err(|e| e.to_string())?;
    parse(std::str::from_utf8(&body).map_err(|_| "LSP body is not UTF-8")?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_roundtrips_unicode_and_frames() {
        let v = Value::object([
            ("text", Value::from("🦀 日本語\n\"\\\0")),
            (
                "a",
                Value::Array(vec![Value::Bool(true), Value::Null, Value::Number(-12.5)]),
            ),
        ]);
        assert_eq!(parse(&v.encode()).unwrap(), v);
        assert_eq!(read_frame(&mut &frame(&v)[..]).unwrap(), v);
        assert_eq!(parse("\"\\ud83e\\udd80\"").unwrap(), Value::from("🦀"));
    }
    #[test]
    fn rejects_malformed_json_and_oversized_frames() {
        for bad in [
            "01",
            "1.",
            "1e",
            "[1,]",
            "{\"x\":1,\"x\":2}",
            "\"\\ud800\"",
            "true false",
            "1e999",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(read_frame(&mut &b"Content-Length: 999999999\r\n\r\n"[..]).is_err());
        assert!(read_frame(&mut &b"Content-Length: 1\r\nContent-Length: 1\r\n\r\n0"[..]).is_err());
    }
}
