//! Just enough JSON to be careful with.
//!
//! The server forwards the CLI's document to the browser verbatim -- the page
//! does the reading, and re-serialising it here would only add a way for the
//! two to disagree. So this parser exists for exactly one job: proving that a
//! node id in a POST body is a node id the read model just returned.
//!
//! That job is a security boundary (invariant 1: a name in a request is a
//! candidate, a name in `copal fleet state` has been through the certificate
//! check), so it is a real parser rather than a substring search for
//! `"id": "..."`. A substring search finds ids inside the stranger list, inside
//! a log line, inside anything an attacker can get echoed into the document.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(BTreeMap<String, Value>),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.get(key),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Escape a string as a JSON literal, quotes included.
    pub fn quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }
}

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn parse(src: &str) -> Result<Value, Error> {
    let b: Vec<char> = src.chars().collect();
    let mut p = Parser { b: &b, i: 0, depth: 0 };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.b.len() {
        return Err(Error(format!("trailing input at char {}", p.i)));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [char],
    i: usize,
    depth: usize,
}

// A document arrives from a subprocess this server launched, so it is not
// hostile -- but the POST body is, and both go through here. Bounded depth
// keeps a nest of ten thousand brackets from ending the process on the stack.
const MAX_DEPTH: usize = 64;

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: char) -> Result<(), Error> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(Error(format!("expected {:?} at char {}", c, self.i)))
        }
    }

    fn lit(&mut self, word: &str) -> Result<(), Error> {
        for c in word.chars() {
            self.eat(c)?;
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Value, Error> {
        if self.depth > MAX_DEPTH {
            return Err(Error("nested too deeply".into()));
        }
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(Value::Str(self.string()?)),
            Some('t') => {
                self.lit("true")?;
                Ok(Value::Bool(true))
            }
            Some('f') => {
                self.lit("false")?;
                Ok(Value::Bool(false))
            }
            Some('n') => {
                self.lit("null")?;
                Ok(Value::Null)
            }
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(Error(format!("unexpected {:?} at char {}", c, self.i))),
            None => Err(Error("input ended early".into())),
        }
    }

    fn object(&mut self) -> Result<Value, Error> {
        self.eat('{')?;
        self.depth += 1;
        let mut map = BTreeMap::new();
        self.ws();
        if self.peek() == Some('}') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Obj(map));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            self.eat(':')?;
            self.ws();
            let v = self.value()?;
            map.insert(k, v);
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some('}') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(Error(format!("expected , or }} at char {}", self.i))),
            }
        }
        self.depth -= 1;
        Ok(Value::Obj(map))
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.eat('[')?;
        self.depth += 1;
        let mut out = Vec::new();
        self.ws();
        if self.peek() == Some(']') {
            self.i += 1;
            self.depth -= 1;
            return Ok(Value::Arr(out));
        }
        loop {
            self.ws();
            out.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(',') => self.i += 1,
                Some(']') => {
                    self.i += 1;
                    break;
                }
                _ => return Err(Error(format!("expected , or ] at char {}", self.i))),
            }
        }
        self.depth -= 1;
        Ok(Value::Arr(out))
    }

    fn string(&mut self) -> Result<String, Error> {
        self.eat('"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(Error("string never closed".into())),
                Some('"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some('\\') => {
                    self.i += 1;
                    let c = self.peek().ok_or_else(|| Error("escape at end".into()))?;
                    self.i += 1;
                    match c {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let mut n = 0u32;
                            for _ in 0..4 {
                                let h = self.peek().ok_or_else(|| Error("short \\u".into()))?;
                                n = n * 16
                                    + h.to_digit(16)
                                        .ok_or_else(|| Error("bad \\u digit".into()))?;
                                self.i += 1;
                            }
                            // A lone surrogate is not a char; U+FFFD keeps the
                            // parse going without inventing a code point.
                            out.push(char::from_u32(n).unwrap_or('\u{fffd}'));
                        }
                        c => return Err(Error(format!("bad escape \\{}", c))),
                    }
                }
                Some(c) => {
                    self.i += 1;
                    out.push(c);
                }
            }
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        if self.peek() == Some('-') {
            self.i += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
        {
            self.i += 1;
        }
        let s: String = self.b[start..self.i].iter().collect();
        s.parse::<f64>()
            .map(Value::Num)
            .map_err(|_| Error(format!("not a number: {}", s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_shapes_the_cli_emits() {
        let v = parse(r#"{"a":1,"b":[true,null,"x"],"c":{"d":-2.5e2}}"#).unwrap();
        assert_eq!(v.get("a"), Some(&Value::Num(1.0)));
        assert_eq!(v.get("b").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(
            v.get("c").unwrap().get("d"),
            Some(&Value::Num(-250.0))
        );
    }

    #[test]
    fn reads_escapes() {
        let v = parse(r#"{"s":"a\"b\\c\nd\u0041"}"#).unwrap();
        assert_eq!(v.get("s").unwrap().as_str(), Some("a\"b\\c\ndA"));
    }

    #[test]
    fn refuses_junk() {
        for bad in [
            "{", "[1,]", "{\"a\"}", "tru", "\"unterminated", "{} {}", "",
        ] {
            assert!(parse(bad).is_err(), "accepted {:?}", bad);
        }
    }

    #[test]
    fn refuses_a_deep_nest_instead_of_dying_on_the_stack() {
        let deep = "[".repeat(5000) + &"]".repeat(5000);
        assert!(parse(&deep).is_err());
    }

    #[test]
    fn quotes_for_the_wire() {
        assert_eq!(Value::quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(Value::quote("l1\nl2"), "\"l1\\nl2\"");
        assert_eq!(Value::quote("\u{1}"), "\"\\u0001\"");
    }
}
