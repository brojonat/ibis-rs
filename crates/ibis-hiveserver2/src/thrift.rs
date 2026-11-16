//! The Thrift binary protocol, untyped: a value is a tree, a struct is a list
//! of numbered fields. Enough to write requests and read replies without
//! generated code.

use std::io::{self, Read};

pub const BOOL: u8 = 2;
pub const BYTE: u8 = 3;
pub const DOUBLE: u8 = 4;
pub const I16: u8 = 6;
pub const I32: u8 = 8;
pub const I64: u8 = 10;
pub const STRING: u8 = 11;
pub const STRUCT: u8 = 12;
pub const MAP: u8 = 13;
pub const SET: u8 = 14;
pub const LIST: u8 = 15;

const VERSION_1: u32 = 0x8001_0000;
const CALL: u32 = 1;
const REPLY: u8 = 2;
const EXCEPTION: u8 = 3;

/// A reply longer than this is a sign of reading the wrong protocol, not of
/// a large result: results arrive a page at a time.
const MAX_CONTAINER: usize = 1 << 28;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Byte(i8),
    Double(f64),
    I16(i16),
    I32(i32),
    I64(i64),
    /// `string` and `binary` are one type on the wire.
    Binary(Vec<u8>),
    Struct(Vec<(i16, Value)>),
    /// Key type, value type, entries.
    Map(u8, u8, Vec<(Value, Value)>),
    /// Element type, elements.
    List(u8, Vec<Value>),
}

pub fn string(s: &str) -> Value {
    Value::Binary(s.as_bytes().to_vec())
}

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

impl Value {
    fn ttype(&self) -> u8 {
        match self {
            Value::Bool(_) => BOOL,
            Value::Byte(_) => BYTE,
            Value::Double(_) => DOUBLE,
            Value::I16(_) => I16,
            Value::I32(_) => I32,
            Value::I64(_) => I64,
            Value::Binary(_) => STRING,
            Value::Struct(_) => STRUCT,
            Value::Map(..) => MAP,
            Value::List(..) => LIST,
        }
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Bool(b) => out.push(*b as u8),
            Value::Byte(b) => out.push(*b as u8),
            Value::Double(f) => out.extend(f.to_be_bytes()),
            Value::I16(i) => out.extend(i.to_be_bytes()),
            Value::I32(i) => out.extend(i.to_be_bytes()),
            Value::I64(i) => out.extend(i.to_be_bytes()),
            Value::Binary(b) => {
                out.extend((b.len() as i32).to_be_bytes());
                out.extend(b);
            }
            Value::Struct(fields) => {
                for (id, v) in fields {
                    out.push(v.ttype());
                    out.extend(id.to_be_bytes());
                    v.write(out);
                }
                out.push(0);
            }
            Value::Map(k, v, entries) => {
                out.extend([*k, *v]);
                out.extend((entries.len() as i32).to_be_bytes());
                for (key, value) in entries {
                    key.write(out);
                    value.write(out);
                }
            }
            Value::List(t, items) => {
                out.push(*t);
                out.extend((items.len() as i32).to_be_bytes());
                for item in items {
                    item.write(out);
                }
            }
        }
    }

    pub fn read(r: &mut impl Read, ttype: u8) -> io::Result<Value> {
        Ok(match ttype {
            BOOL => Value::Bool(bytes::<1>(r)?[0] != 0),
            BYTE => Value::Byte(bytes::<1>(r)?[0] as i8),
            DOUBLE => Value::Double(f64::from_be_bytes(bytes(r)?)),
            I16 => Value::I16(i16::from_be_bytes(bytes(r)?)),
            I32 => Value::I32(i32::from_be_bytes(bytes(r)?)),
            I64 => Value::I64(i64::from_be_bytes(bytes(r)?)),
            STRING => {
                let mut buf = vec![0; size(r)?];
                r.read_exact(&mut buf)?;
                Value::Binary(buf)
            }
            STRUCT => {
                let mut fields = vec![];
                loop {
                    let t = bytes::<1>(r)?[0];
                    if t == 0 {
                        break Value::Struct(fields);
                    }
                    let id = i16::from_be_bytes(bytes(r)?);
                    fields.push((id, Value::read(r, t)?));
                }
            }
            MAP => {
                let [k, v] = bytes(r)?;
                let n = size(r)?;
                let mut entries = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    entries.push((Value::read(r, k)?, Value::read(r, v)?));
                }
                Value::Map(k, v, entries)
            }
            SET | LIST => {
                let t = bytes::<1>(r)?[0];
                let n = size(r)?;
                let mut items = Vec::with_capacity(n.min(1 << 16));
                for _ in 0..n {
                    items.push(Value::read(r, t)?);
                }
                Value::List(t, items)
            }
            other => return Err(bad(format!("unknown Thrift type {other}"))),
        })
    }

    /// Field `id` of a struct.
    pub fn field(&self, id: i16) -> Option<&Value> {
        match self {
            Value::Struct(fields) => fields.iter().find(|(i, _)| *i == id).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Value::I32(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Binary(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        self.as_bytes().and_then(|b| std::str::from_utf8(b).ok())
    }

    pub fn as_list(&self) -> &[Value] {
        match self {
            Value::List(_, items) => items,
            _ => &[],
        }
    }

    pub fn as_map(&self) -> &[(Value, Value)] {
        match self {
            Value::Map(_, _, entries) => entries,
            _ => &[],
        }
    }
}

fn bytes<const N: usize>(r: &mut impl Read) -> io::Result<[u8; N]> {
    let mut buf = [0; N];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn size(r: &mut impl Read) -> io::Result<usize> {
    let n = i32::from_be_bytes(bytes(r)?);
    if n < 0 || n as usize > MAX_CONTAINER {
        return Err(bad(format!("implausible length {n}")));
    }
    Ok(n as usize)
}

/// The message calling `method` with one argument struct, `request`.
pub fn call(method: &str, seq: i32, request: Value) -> Vec<u8> {
    let mut out = vec![];
    out.extend((VERSION_1 | CALL).to_be_bytes());
    string(method).write(&mut out);
    out.extend(seq.to_be_bytes());
    Value::Struct(vec![(1, request)]).write(&mut out);
    out
}

/// Read the reply to a call: the result struct's `success` field.
pub fn reply(r: &mut impl Read) -> io::Result<Value> {
    let header = u32::from_be_bytes(bytes(r)?);
    if header & 0xffff_0000 != VERSION_1 {
        return Err(bad(format!("not a Thrift binary reply (it starts {header:#010x})")));
    }
    let method = Value::read(r, STRING)?;
    let _seq: [u8; 4] = bytes(r)?;
    let body = Value::read(r, STRUCT)?;
    match (header & 0xff) as u8 {
        REPLY => body.field(0).cloned().ok_or_else(|| bad("reply without a result")),
        // TApplicationException: 1 is the message
        EXCEPTION => Err(io::Error::other(format!(
            "{}: {}",
            method.as_str().unwrap_or("call"),
            body.field(1).and_then(Value::as_str).unwrap_or("the server raised an exception")
        ))),
        other => Err(bad(format!("unexpected message type {other}"))),
    }
}
