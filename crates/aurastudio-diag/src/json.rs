//! A small JSON reader, enough to read a report back (`--baseline`):
//! objects, arrays, strings, numbers, true / false / null.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(s: &str) -> Option<Json> {
        let mut p = Parser { b: s.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        (p.i == p.b.len()).then_some(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self { Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v), _ => None }
    }

    pub fn as_f64(&self) -> Option<f64> { match self { Json::Num(n) => Some(*n), _ => None } }
    pub fn as_str(&self) -> Option<&str> { match self { Json::Str(s) => Some(s), _ => None } }
    pub fn as_arr(&self) -> &[Json] { match self { Json::Arr(a) => a, _ => &[] } }
}

struct Parser<'a> { b: &'a [u8], i: usize }

impl Parser<'_> {
    fn ws(&mut self) { while self.b.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) { self.i += 1; } }

    fn eat(&mut self, c: u8) -> Option<()> { self.ws(); if self.b.get(self.i) == Some(&c) { self.i += 1; Some(()) } else { None } }

    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut kv = Vec::new();
                if self.eat(b'}').is_some() { return Some(Json::Obj(kv)); }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    kv.push((k, self.value()?));
                    if self.eat(b',').is_some() { continue; }
                    self.eat(b'}')?;
                    return Some(Json::Obj(kv));
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if self.eat(b']').is_some() { return Some(Json::Arr(a)); }
                loop {
                    a.push(self.value()?);
                    if self.eat(b',').is_some() { continue; }
                    self.eat(b']')?;
                    return Some(Json::Arr(a));
                }
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.word("true", Json::Bool(true)),
            b'f' => self.word("false", Json::Bool(false)),
            b'n' => self.word("null", Json::Null),
            _ => {
                let s = self.i;
                while self.b.get(self.i).is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(c)) { self.i += 1; }
                std::str::from_utf8(&self.b[s..self.i]).ok()?.parse().ok().map(Json::Num)
            }
        }
    }

    fn word(&mut self, w: &str, v: Json) -> Option<Json> {
        if self.b.get(self.i..self.i + w.len())? == w.as_bytes() { self.i += w.len(); Some(v) } else { None }
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') { return None; }
        self.i += 1;
        let mut out = String::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push('\n'), b't' => out.push('\t'), b'r' => out.push('\r'),
                        b'u' => {
                            let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            out.push(char::from_u32(u32::from_str_radix(h, 16).ok()?)?);
                        }
                        other => out.push(other as char),
                    }
                }
                _ => {
                    // (UTF-8 continuation bytes pass through unchanged.)
                    let s = self.i - 1;
                    let len = match c { 0..=0x7f => 1, 0xc0..=0xdf => 2, 0xe0..=0xef => 3, _ => 4 };
                    out.push_str(std::str::from_utf8(self.b.get(s..s + len)?).ok()?);
                    self.i = s + len;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_reports_write() {
        let j = Json::parse(r#"{"a": [1, -2.5e3, true, null], "b": {"c": "x\"yéé"}}"#).expect("json");
        assert_eq!(j.get("a").map(|a| a.as_arr().len()), Some(4));
        assert_eq!(j.get("a").and_then(|a| a.as_arr()[1].as_f64()), Some(-2500.0));
        assert_eq!(j.get("b").and_then(|b| b.get("c")).and_then(Json::as_str), Some("x\"yéé"));
        assert!(Json::parse("{\"a\": }").is_none());
    }
}
