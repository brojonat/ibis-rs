//! Scalar values: literals in expressions, and the row representation that
//! backends without native Arrow output convert through.

use crate::datatypes::DataType;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Binary(Vec<u8>),
    /// Days since the Unix epoch.
    Date(i32),
    /// Microseconds since the Unix epoch.
    Timestamp(i64),
    Array(Vec<Value>),
    /// Fields in order, by name.
    Struct(Vec<(String, Value)>),
    /// Entries in order.
    Map(Vec<(Value, Value)>),
}

impl Value {
    /// The dtype a literal of this value gets.
    pub fn dtype(&self) -> DataType {
        match self {
            Value::Null => DataType::Null,
            Value::Bool(_) => DataType::Boolean,
            // Ibis: the smallest integer type that holds the value
            Value::Int(i) => DataType::smallest_int(*i as i128),
            Value::Float(_) => DataType::Float64,
            Value::String(_) => DataType::String,
            Value::Binary(_) => DataType::Binary,
            Value::Date(_) => DataType::Date,
            Value::Timestamp(_) => DataType::Timestamp { tz: None },
            // Ibis: the type every element fits in; no elements, no type
            Value::Array(items) => DataType::array(common(items.iter())),
            Value::Struct(fields) => DataType::Struct(fields.iter().map(|(n, v)| (n.clone(), v.dtype())).collect()),
            Value::Map(entries) => {
                DataType::map(common(entries.iter().map(|(k, _)| k)), common(entries.iter().map(|(_, v)| v)))
            }
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

/// The type all of the values fit in; string if they have nothing in common.
fn common<'a>(values: impl Iterator<Item = &'a Value>) -> DataType {
    values.map(Value::dtype).try_fold(DataType::Null, |acc, t| DataType::unify(&acc, &t)).unwrap_or(DataType::String)
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Self { Value::Int(v as i64) }
        }
    )*};
}
from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Float(v as f64)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::String(v)
    }
}
impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Value>, const N: usize> From<[T; N]> for Value {
    fn from(v: [T; N]) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Null, Into::into)
    }
}
