//! Binary KeyValues as used by `shortcuts.vdf`.
//!
//! Keys and strings are kept as raw bytes and unknown value types as raw payloads,
//! so rewriting a file never alters entries we didn't touch.

const T_OBJ: u8 = 0x00;
const T_STR: u8 = 0x01;
const T_INT: u8 = 0x02;
const T_FLOAT: u8 = 0x03;
const T_PTR: u8 = 0x04;
const T_WSTR: u8 = 0x05;
const T_COLOR: u8 = 0x06;
const T_U64: u8 = 0x07;
const T_END: u8 = 0x08;
const T_I64: u8 = 0x0A;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Obj(Obj),
    Str(Vec<u8>),
    Int(i32),
    /// Any other type, stored as (type byte, payload) for lossless round-trips.
    Raw(u8, Vec<u8>),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Obj(pub Vec<(Vec<u8>, Value)>);

impl Obj {
    fn position(&self, key: &str) -> Option<usize> {
        self.0.iter().position(|(k, _)| k.eq_ignore_ascii_case(key.as_bytes()))
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.position(key).map(|i| &self.0[i].1)
    }

    pub fn get_str(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            Value::Str(s) => Some(String::from_utf8_lossy(s).into_owned()),
            _ => None,
        }
    }

    pub fn get_int(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn get_obj(&self, key: &str) -> Option<&Obj> {
        match self.get(key)? {
            Value::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn obj_mut(&mut self, key: &str) -> &mut Obj {
        let i = match self.position(key) {
            Some(i) if matches!(self.0[i].1, Value::Obj(_)) => i,
            Some(i) => {
                self.0[i].1 = Value::Obj(Obj::default());
                i
            }
            None => {
                self.0.push((key.as_bytes().to_vec(), Value::Obj(Obj::default())));
                self.0.len() - 1
            }
        };
        match &mut self.0[i].1 {
            Value::Obj(o) => o,
            _ => unreachable!(),
        }
    }

    pub fn set(&mut self, key: &str, value: Value) {
        match self.position(key) {
            Some(i) => self.0[i].1 = value,
            None => self.0.push((key.as_bytes().to_vec(), value)),
        }
    }

    pub fn set_str(&mut self, key: &str, value: &str) {
        self.set(key, Value::Str(value.as_bytes().to_vec()));
    }

    pub fn set_int(&mut self, key: &str, value: i32) {
        self.set(key, Value::Int(value));
    }
}

pub fn parse(data: &[u8]) -> Result<Obj, String> {
    let mut r = Reader { d: data, i: 0 };
    let obj = r.obj_body(true)?;
    Ok(obj)
}

pub fn serialize(root: &Obj) -> Vec<u8> {
    let mut out = Vec::new();
    write_obj(&mut out, root);
    out
}

fn write_obj(out: &mut Vec<u8>, obj: &Obj) {
    for (k, v) in &obj.0 {
        let (ty, payload): (u8, Vec<u8>) = match v {
            Value::Obj(_) => (T_OBJ, Vec::new()),
            Value::Str(s) => (T_STR, [s.as_slice(), &[0]].concat()),
            Value::Int(n) => (T_INT, n.to_le_bytes().to_vec()),
            Value::Raw(t, p) => (*t, p.clone()),
        };
        out.push(ty);
        out.extend_from_slice(k);
        out.push(0);
        match v {
            Value::Obj(o) => write_obj(out, o),
            _ => out.extend_from_slice(&payload),
        }
    }
    out.push(T_END);
}

struct Reader<'a> {
    d: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn obj_body(&mut self, top: bool) -> Result<Obj, String> {
        let mut obj = Obj::default();
        loop {
            let Some(&ty) = self.d.get(self.i) else {
                // Tolerate files missing the final terminator.
                return if top { Ok(obj) } else { Err("unexpected end of data".into()) };
            };
            self.i += 1;
            if ty == T_END {
                return Ok(obj);
            }
            let key = self.cstr()?;
            let value = match ty {
                T_OBJ => Value::Obj(self.obj_body(false)?),
                T_STR => Value::Str(self.cstr()?),
                T_INT => Value::Int(i32::from_le_bytes(self.take(4)?.try_into().unwrap())),
                T_FLOAT | T_PTR | T_COLOR => Value::Raw(ty, self.take(4)?.to_vec()),
                T_U64 | T_I64 => Value::Raw(ty, self.take(8)?.to_vec()),
                T_WSTR => Value::Raw(ty, self.wstr()?),
                other => return Err(format!("unknown value type 0x{other:02x} at byte {}", self.i)),
            };
            obj.0.push((key, value));
        }
    }

    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.i.checked_add(n).filter(|&e| e <= self.d.len()).ok_or("truncated value")?;
        let s = &self.d[self.i..end];
        self.i = end;
        Ok(s)
    }

    fn cstr(&mut self) -> Result<Vec<u8>, String> {
        let rel = self.d[self.i..].iter().position(|&b| b == 0).ok_or("unterminated string")?;
        let s = self.d[self.i..self.i + rel].to_vec();
        self.i += rel + 1;
        Ok(s)
    }

    /// UTF-16 string terminated by a 0x0000 code unit; payload includes the terminator.
    fn wstr(&mut self) -> Result<Vec<u8>, String> {
        let start = self.i;
        loop {
            let unit = self.take(2)?;
            if unit == [0, 0] {
                return Ok(self.d[start..self.i].to_vec());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"\x00shortcuts\x00");
        d.extend_from_slice(b"\x000\x00");
        d.extend_from_slice(b"\x02appid\x00\x15\xcd\x5b\x87");
        d.extend_from_slice(b"\x01AppName\x00Some Game\x00");
        d.extend_from_slice(b"\x01Exe\x00\"/usr/bin/game\"\x00");
        d.extend_from_slice(b"\x07Weird\x00\x01\x02\x03\x04\x05\x06\x07\x08");
        d.extend_from_slice(b"\x00tags\x00\x010\x00favorite\x00\x08");
        d.extend_from_slice(b"\x08");
        d.extend_from_slice(b"\x08\x08");
        d
    }

    #[test]
    fn round_trips_byte_for_byte() {
        let data = sample();
        let root = parse(&data).unwrap();
        assert_eq!(serialize(&root), data);
    }

    #[test]
    fn reads_fields() {
        let root = parse(&sample()).unwrap();
        let entry = root.get_obj("shortcuts").unwrap().get_obj("0").unwrap();
        assert_eq!(entry.get_str("appname").as_deref(), Some("Some Game"));
        assert_eq!(entry.get_int("appid"), Some(0x875bcd15u32 as i32));
        assert_eq!(entry.get_obj("tags").unwrap().get_str("0").as_deref(), Some("favorite"));
    }

    #[test]
    fn empty_file_is_empty_root() {
        assert_eq!(parse(&[]).unwrap(), Obj::default());
    }

    #[test]
    fn rejects_truncated() {
        assert!(parse(b"\x00shortcuts\x00\x02appid\x00\x01").is_err());
    }
}
