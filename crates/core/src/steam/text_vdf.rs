//! Minimal, order-preserving reader/writer for Valve's text KeyValues format
//! (`config.vdf`, `loginusers.vdf`, ...).

use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Obj(Obj),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Obj(pub Vec<(String, Value)>);

impl Obj {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }

    pub fn get_obj(&self, key: &str) -> Option<&Obj> {
        match self.get(key)? {
            Value::Obj(o) => Some(o),
            Value::Str(_) => None,
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Value::Str(s) => Some(s),
            Value::Obj(_) => None,
        }
    }

    /// Get the child object `key`, creating it (or replacing a string of that name).
    pub fn obj_mut(&mut self, key: &str) -> &mut Obj {
        let idx = match self.0.iter().position(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some(i) => {
                if matches!(self.0[i].1, Value::Str(_)) {
                    self.0[i].1 = Value::Obj(Obj::default());
                }
                i
            }
            None => {
                self.0.push((key.to_owned(), Value::Obj(Obj::default())));
                self.0.len() - 1
            }
        };
        match &mut self.0[idx].1 {
            Value::Obj(o) => o,
            Value::Str(_) => unreachable!(),
        }
    }

    /// Set `key`, replacing an existing entry in place to keep ordering stable.
    pub fn set(&mut self, key: &str, value: Value) {
        match self.0.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }

    pub fn set_str(&mut self, key: &str, value: &str) {
        self.set(key, Value::Str(value.to_owned()));
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.0.iter().position(|(k, _)| k.eq_ignore_ascii_case(key))?;
        Some(self.0.remove(i).1)
    }
}

pub fn parse(src: &str) -> Result<Obj, String> {
    let mut p = Parser { s: src.as_bytes(), i: 0 };
    let root = p.parse_obj_body(true)?;
    Ok(root)
}

pub fn serialize(root: &Obj) -> String {
    let mut out = String::new();
    write_obj(&mut out, root, 0);
    out
}

fn write_obj(out: &mut String, obj: &Obj, depth: usize) {
    let indent = "\t".repeat(depth);
    for (k, v) in &obj.0 {
        match v {
            Value::Str(s) => {
                let _ = writeln!(out, "{indent}\"{}\"\t\t\"{}\"", escape(k), escape(s));
            }
            Value::Obj(o) => {
                let _ = writeln!(out, "{indent}\"{}\"", escape(k));
                let _ = writeln!(out, "{indent}{{");
                write_obj(out, o, depth + 1);
                let _ = writeln!(out, "{indent}}}");
            }
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

enum Tok {
    Str(String),
    Open,
    Close,
    Eof,
}

impl Parser<'_> {
    fn parse_obj_body(&mut self, top: bool) -> Result<Obj, String> {
        let mut obj = Obj::default();
        loop {
            let key = match self.next()? {
                Tok::Str(k) => k,
                Tok::Close if !top => return Ok(obj),
                Tok::Eof if top => return Ok(obj),
                Tok::Eof => return Err("unexpected end of file".into()),
                Tok::Open | Tok::Close => return Err(format!("unexpected brace at byte {}", self.i)),
            };
            let value = match self.next()? {
                Tok::Str(v) => Value::Str(v),
                Tok::Open => Value::Obj(self.parse_obj_body(false)?),
                _ => return Err(format!("missing value for key {key:?}")),
            };
            obj.0.push((key, value));
        }
    }

    fn next(&mut self) -> Result<Tok, String> {
        loop {
            self.skip_ws();
            let Some(&c) = self.s.get(self.i) else { return Ok(Tok::Eof) };
            match c {
                b'/' if self.s.get(self.i + 1) == Some(&b'/') => self.skip_line(),
                // Platform conditionals like [$WIN32] are ignored.
                b'[' => {
                    while self.s.get(self.i).is_some_and(|&c| c != b']') {
                        self.i += 1;
                    }
                    self.i += 1;
                }
                b'{' => {
                    self.i += 1;
                    return Ok(Tok::Open);
                }
                b'}' => {
                    self.i += 1;
                    return Ok(Tok::Close);
                }
                b'"' => return self.quoted().map(Tok::Str),
                _ => return Ok(Tok::Str(self.bare())),
            }
        }
    }

    fn skip_ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn skip_line(&mut self) {
        while self.s.get(self.i).is_some_and(|&c| c != b'\n') {
            self.i += 1;
        }
    }

    fn quoted(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut buf = Vec::new();
        loop {
            match self.s.get(self.i) {
                None => return Err("unterminated string".into()),
                Some(b'"') => {
                    self.i += 1;
                    break;
                }
                Some(b'\\') => {
                    let esc = *self.s.get(self.i + 1).ok_or("unterminated escape")?;
                    buf.push(match esc {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                    self.i += 2;
                }
                Some(&c) => {
                    buf.push(c);
                    self.i += 1;
                }
            }
        }
        String::from_utf8(buf).map_err(|_| "invalid UTF-8 in string".into())
    }

    fn bare(&mut self) -> String {
        let start = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|&c| !c.is_ascii_whitespace() && !matches!(c, b'{' | b'}' | b'"'))
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"BaseInstallFolder_1\"\t\t\"/run/media/deck/SD\"\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"0\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"75\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n";

    #[test]
    fn round_trips_steam_formatting() {
        let parsed = parse(CONFIG).unwrap();
        assert_eq!(serialize(&parsed), CONFIG);
    }

    #[test]
    fn case_insensitive_navigation() {
        let root = parse(CONFIG).unwrap();
        let steam = root
            .get_obj("installconfigstore")
            .and_then(|o| o.get_obj("Software"))
            .and_then(|o| o.get_obj("Valve"))
            .and_then(|o| o.get_obj("steam"))
            .unwrap();
        assert_eq!(steam.get_str("basEinstallfolder_1"), Some("/run/media/deck/SD"));
    }

    #[test]
    fn escapes_comments_and_conditionals() {
        let src = "// comment\n\"a\" { \"p\" \"C:\\\\x \\\"q\\\"\" [$WIN32] \"b\" \"1\" }";
        let root = parse(src).unwrap();
        let a = root.get_obj("a").unwrap();
        assert_eq!(a.get_str("p"), Some("C:\\x \"q\""));
        assert_eq!(a.get_str("b"), Some("1"));
        let again = parse(&serialize(&root)).unwrap();
        assert_eq!(again, root);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("\"a\" { \"b\" ").is_err());
        assert!(parse("}").is_err());
    }
}
