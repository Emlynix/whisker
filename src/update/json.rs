//! Minimal JSON reader for release manifests. Hostile input never panics:
//! bad syntax, deep nesting or bad escapes give `None`.

/// Deepest nesting accepted; manifests need 4.
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    /// Number as written, e.g. `"12.5e3"` (manifests need none).
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Member of an object, e.g. `j.get("version")`.
    pub fn get(&self, key: &str) -> Option<&Json> {
        let Json::Obj(members) = self else {
            return None;
        };
        members.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn items(&self) -> &[Json] {
        match self {
            Json::Arr(a) => a,
            _ => &[],
        }
    }
}

/// Whole text as one value; trailing garbage is an error.
pub fn parse(text: &str) -> Option<Json> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    (p.i == p.s.len()).then_some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Option<()> {
        self.ws();
        if self.peek()? != c {
            return None;
        }
        self.i += 1;
        Some(())
    }

    fn word(&mut self, w: &str, v: Json) -> Option<Json> {
        if !self.s.get(self.i..)?.starts_with(w.as_bytes()) {
            return None;
        }
        self.i += w.len();
        Some(v)
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.ws();
        match self.peek()? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(Json::Str),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'n' => self.word("null", Json::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn object(&mut self, depth: usize) -> Option<Json> {
        self.i += 1;
        let mut members = Vec::new();
        self.ws();
        if self.peek()? == b'}' {
            self.i += 1;
            return Some(Json::Obj(members));
        }
        loop {
            self.ws();
            if self.peek()? != b'"' {
                return None;
            }
            let k = self.string()?;
            self.eat(b':')?;
            members.push((k, self.value(depth + 1)?));
            self.ws();
            match self.peek()? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return Some(Json::Obj(members));
                }
                _ => return None,
            }
        }
    }

    fn array(&mut self, depth: usize) -> Option<Json> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.peek()? == b']' {
            self.i += 1;
            return Some(Json::Arr(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek()? {
                b',' => self.i += 1,
                b']' => {
                    self.i += 1;
                    return Some(Json::Arr(items));
                }
                _ => return None,
            }
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        while matches!(self.peek(), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
            self.i += 1;
        }
        let raw = std::str::from_utf8(&self.s[start..self.i]).ok()?;
        raw.parse::<f64>().ok()?;
        Some(Json::Num(raw.to_string()))
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.s.get(self.i..self.i + 4)?;
        self.i += 4;
        u32::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok().filter(|_| h.iter().all(u8::is_ascii_hexdigit))
    }

    /// `"…"` with escapes; raw control characters are refused.
    fn string(&mut self) -> Option<String> {
        const SURROGATE_HI: std::ops::Range<u32> = 0xD800..0xDC00;
        const SURROGATE_LO: std::ops::Range<u32> = 0xDC00..0xE000;
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                0..=0x1F => return None,
                b'\\' => {
                    let e = self.peek()?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            let code = if SURROGATE_HI.contains(&hi) {
                                if self.s.get(self.i..self.i + 2)? != b"\\u" {
                                    return None;
                                }
                                self.i += 2;
                                let lo = self.hex4().filter(|lo| SURROGATE_LO.contains(lo))?;
                                0x10000 + ((hi - SURROGATE_HI.start) << 10) + (lo - SURROGATE_LO.start)
                            } else {
                                hi
                            };
                            char::from_u32(code)?
                        }
                        _ => return None,
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nested_values() {
        let j = parse(r#" {"a": [1, -2.5e3, true, null], "b": {"c": "x\"y\u00e9\n"}} "#).unwrap();
        assert_eq!(j.get("a").unwrap().items().len(), 4);
        assert_eq!(j.get("b").and_then(|b| b.get("c")).and_then(Json::str), Some("x\"yé\n"));
        assert_eq!(j.get("missing"), None);
    }

    #[test]
    fn surrogate_pairs_decode() {
        assert_eq!(parse(r#""\ud83d\ude00""#), Some(Json::Str("😀".into())));
        assert_eq!(parse(r#""\ud83d""#), None);
        assert_eq!(parse(r#""\ude00""#), None);
    }

    #[test]
    fn hostile_input_is_refused() {
        let bad = [
            "", "{", "[1,", r#"{"a" 1}"#, r#"{"a":1,}"#, "[1 2]", "tru", "\"abc", "\"a\u{1}b\"", r#""\x""#, r#""\u12""#,
            r#""\u+123""#, "1 2", "{}x", "--1", "\"\\", "[", "nul",
        ];
        for s in bad {
            assert_eq!(parse(s), None, "{s:?}");
        }
        let deep = "[".repeat(10_000) + &"]".repeat(10_000);
        assert_eq!(parse(&deep), None);
    }

    #[test]
    fn every_prefix_is_safe() {
        let text = r#"{"version":"0.9.1","assets":{"windows-x86_64":{"url":"https://x/y","sha256":"ab"}},"n":[1,2.0,"\u00e9"]}"#;
        for i in 0..text.len() {
            let _ = parse(&text[..i]);
        }
        assert!(parse(text).is_some());
    }
}
