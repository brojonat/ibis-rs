//! Nested values as JSON: how several engines hand arrays, structs and maps
//! back, and how they take them in.

use ibis_core::convert::{format_date, format_timestamp};
use ibis_core::{DataType, Value};
use serde_json::{Map, Number, Value as Json};

/// A JSON value read as a value of type `t`.
///
/// Arrays are JSON arrays. A struct is an object (fields found by name) or
/// an array (fields by position). A map is an object, whose keys are then
/// strings whatever the key type, or an array of `[key, value]` pairs or of
/// `{"key": .., "value": ..}` objects. Scalars are passed on as they are:
/// building an Arrow column of the output type parses the text of a date,
/// a timestamp or a number.
pub fn value_from_json(j: &Json, t: &DataType) -> Value {
    let null = DataType::Null;
    match (j, t) {
        (Json::Null, _) => Value::Null,
        (Json::Bool(b), _) => Value::Bool(*b),
        (Json::Number(n), DataType::Float32 | DataType::Float64 | DataType::Decimal { .. }) => {
            n.as_f64().map_or(Value::Null, Value::Float)
        }
        (Json::Number(n), _) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => n.as_f64().map_or(Value::Null, Value::Float),
        },
        (Json::String(s), _) => Value::String(s.clone()),
        (Json::Array(items), DataType::Struct(fields)) => Value::Struct(
            fields
                .iter()
                .enumerate()
                .map(|(i, (name, t))| (name.clone(), items.get(i).map_or(Value::Null, |v| value_from_json(v, t))))
                .collect(),
        ),
        (Json::Array(items), DataType::Map(k, v)) => Value::Map(
            items
                .iter()
                .filter_map(|entry| match entry {
                    Json::Array(kv) if kv.len() == 2 => Some((value_from_json(&kv[0], k), value_from_json(&kv[1], v))),
                    Json::Object(o) => Some((
                        value_from_json(o.get("key")?, k),
                        o.get("value").map_or(Value::Null, |x| value_from_json(x, v)),
                    )),
                    _ => None,
                })
                .collect(),
        ),
        (Json::Array(items), _) => {
            let item = t.item().unwrap_or(&null);
            Value::Array(items.iter().map(|v| value_from_json(v, item)).collect())
        }
        (Json::Object(o), DataType::Map(_, v)) => {
            Value::Map(o.iter().map(|(key, x)| (Value::String(key.clone()), value_from_json(x, v))).collect())
        }
        (Json::Object(o), DataType::Struct(fields)) => Value::Struct(
            fields
                .iter()
                .map(|(name, t)| (name.clone(), o.get(name).map_or(Value::Null, |v| value_from_json(v, t))))
                .collect(),
        ),
        (Json::Object(o), _) => {
            Value::Struct(o.iter().map(|(name, v)| (name.clone(), value_from_json(v, &null))).collect())
        }
    }
}

/// JSON text read as a value of type `t`; `None` if it is not JSON.
pub fn parse_json(text: &str, t: &DataType) -> Option<Value> {
    serde_json::from_str::<Json>(text).ok().map(|j| value_from_json(&j, t))
}

/// The text a map key has as the key of a JSON object.
pub fn json_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Date(d) => format_date(*d),
        Value::Timestamp(t) => format_timestamp(*t),
        other => value_to_json(other).to_string(),
    }
}

/// A value as JSON. Dates and timestamps are their ISO text, binary is hex,
/// a float JSON has no number for (NaN, the infinities) is its name, and a
/// map is an object keyed by [`json_key`].
pub fn value_to_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Number((*i).into()),
        Value::Float(f) => match Number::from_f64(*f) {
            Some(n) => Json::Number(n),
            None if f.is_nan() => Json::String("NaN".into()),
            None => Json::String(if *f > 0.0 { "Infinity" } else { "-Infinity" }.into()),
        },
        Value::String(s) => Json::String(s.clone()),
        Value::Binary(b) => Json::String(b.iter().map(|byte| format!("{byte:02x}")).collect()),
        Value::Date(d) => Json::String(format_date(*d)),
        Value::Timestamp(t) => Json::String(format_timestamp(*t)),
        Value::Array(items) => Json::Array(items.iter().map(value_to_json).collect()),
        Value::Struct(fields) => {
            Json::Object(fields.iter().map(|(name, v)| (name.clone(), value_to_json(v))).collect::<Map<_, _>>())
        }
        Value::Map(entries) => {
            Json::Object(entries.iter().map(|(k, v)| (json_key(k), value_to_json(v))).collect::<Map<_, _>>())
        }
    }
}

/// Turn the JSON text an engine sent for a nested column into the value it
/// stands for, wherever in `v` such text sits: `v` itself, or the elements
/// of an array the driver did decode.
pub fn decode_nested(v: Value, t: &DataType) -> Value {
    match (v, t) {
        (Value::String(text), t) if t.is_nested() => parse_json(&text, t).unwrap_or(Value::String(text)),
        (Value::Array(items), DataType::Array(item)) => {
            Value::Array(items.into_iter().map(|v| decode_nested(v, item)).collect())
        }
        (v, _) => v,
    }
}
