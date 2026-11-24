//! Conversions between Arrow batches, rows of `Value`s, and the Ibis output
//! schema. Backends use these to honor the contract that results are shaped
//! like `table.schema()` regardless of what the engine returned natively.

use std::sync::Arc;

use arrow_array::builder::{
    BinaryBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Float32Builder, Float64Builder, Int8Builder,
    Int16Builder, Int32Builder, Int64Builder, StringBuilder, TimestampMicrosecondBuilder, UInt8Builder, UInt16Builder,
    UInt32Builder, UInt64Builder,
};
use arrow_array::cast::AsArray;
use arrow_array::types::*;
use arrow_array::{Array, ArrayRef, ListArray, MapArray, RecordBatch, RecordBatchOptions, StructArray};
use arrow_buffer::{NullBuffer, OffsetBuffer};
use arrow_schema::{DataType as ArrowType, Field as ArrowField, TimeUnit};
use chrono::{Datelike, NaiveDate, NaiveDateTime};

use crate::datatypes::{DataType, Schema};
use crate::error::{Error, Result};
use crate::value::Value;

const EPOCH_DAYS_FROM_CE: i32 = 719_163;

/// Parse `YYYY-MM-DD` to days since epoch.
pub fn parse_date(s: &str) -> Option<i32> {
    NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok().map(|d| d.num_days_from_ce() - EPOCH_DAYS_FROM_CE)
}

/// Parse common timestamp spellings to microseconds since epoch.
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M", "%Y-%m-%d"] {
        if let Ok(t) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(t.and_utc().timestamp_micros());
        }
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_micros())
}

pub fn format_date(days: i32) -> String {
    NaiveDate::from_num_days_from_ce_opt(days + EPOCH_DAYS_FROM_CE)
        .map_or_else(|| days.to_string(), |d| d.format("%Y-%m-%d").to_string())
}

pub fn format_timestamp(micros: i64) -> String {
    match chrono::DateTime::from_timestamp_micros(micros) {
        Some(t) if micros % 1_000_000 == 0 => t.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string(),
        Some(t) => t.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f").to_string(),
        None => micros.to_string(),
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(*b as i64),
        Value::Float(f) if f.fract() == 0.0 => Some(*f as i64),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Int(i) => Some(*i != 0),
        Value::String(s) => match s.as_str() {
            "t" | "true" | "1" => Some(true),
            "f" | "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Int(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Date(d) => Some(format_date(*d)),
        Value::Timestamp(t) => Some(format_timestamp(*t)),
        _ => None,
    }
}

fn as_date(v: &Value) -> Option<i32> {
    match v {
        Value::Date(d) => Some(*d),
        Value::Timestamp(t) => Some(t.div_euclid(86_400_000_000) as i32),
        Value::String(s) => parse_date(s),
        _ => None,
    }
}

fn as_ts(v: &Value) -> Option<i64> {
    match v {
        Value::Timestamp(t) => Some(*t),
        Value::Date(d) => Some(*d as i64 * 86_400_000_000),
        Value::String(s) => parse_timestamp(s),
        _ => None,
    }
}

/// Build one Arrow column of `dtype` from values, coercing engine-native
/// representations (SQLite's 0/1 booleans, text timestamps, ...).
pub fn build_array(dtype: &DataType, vals: &[Value]) -> Result<ArrayRef> {
    macro_rules! build {
        ($b:ty, $conv:expr) => {{
            let mut b = <$b>::new();
            for v in vals {
                if v.is_null() {
                    b.append_null();
                } else {
                    match $conv(v) {
                        Some(x) => b.append_value(x),
                        None => return Err(Error::Other(format!("can't convert {v:?} to {dtype}"))),
                    }
                }
            }
            Arc::new(b.finish()) as ArrayRef
        }};
    }
    Ok(match dtype {
        // what moves a date, not yet a value a column holds
        DataType::Interval(_) => return Err(Error::Other(format!("no column of {dtype} values yet"))),
        DataType::Null => Arc::new(arrow_array::NullArray::new(vals.len())),
        DataType::Boolean => build!(BooleanBuilder, as_bool),
        DataType::Int8 => build!(Int8Builder, |v| as_i64(v).map(|i| i as i8)),
        DataType::Int16 => build!(Int16Builder, |v| as_i64(v).map(|i| i as i16)),
        DataType::Int32 => build!(Int32Builder, |v| as_i64(v).map(|i| i as i32)),
        DataType::Int64 => build!(Int64Builder, as_i64),
        DataType::UInt8 => build!(UInt8Builder, |v| as_i64(v).map(|i| i as u8)),
        DataType::UInt16 => build!(UInt16Builder, |v| as_i64(v).map(|i| i as u16)),
        DataType::UInt32 => build!(UInt32Builder, |v| as_i64(v).map(|i| i as u32)),
        DataType::UInt64 => build!(UInt64Builder, |v| as_i64(v).map(|i| i as u64)),
        DataType::Float32 => build!(Float32Builder, |v| as_f64(v).map(|f| f as f32)),
        DataType::Float64 => build!(Float64Builder, as_f64),
        DataType::Decimal { precision, scale } => {
            let mul = 10f64.powi(*scale as i32);
            let a = build!(Decimal128Builder, |v| as_f64(v).map(|f| (f * mul).round() as i128));
            Arc::new(a.as_primitive::<Decimal128Type>().clone().with_precision_and_scale(*precision, *scale)?)
        }
        DataType::String => build!(StringBuilder, as_string),
        DataType::Binary => build!(BinaryBuilder, |v: &Value| match v {
            Value::Binary(b) => Some(b.clone()),
            Value::String(s) => Some(s.as_bytes().to_vec()),
            _ => None,
        }),
        DataType::Date => build!(Date32Builder, as_date),
        DataType::Timestamp { tz } => {
            let a = build!(TimestampMicrosecondBuilder, as_ts);
            Arc::new(a.as_primitive::<TimestampMicrosecondType>().clone().with_timezone_opt(tz.clone()))
        }
        DataType::Array(item) => {
            let lists = vals.iter().map(|v| as_items(v, dtype)).collect::<Result<Vec<_>>>()?;
            let flat: Vec<Value> = lists.iter().flatten().flat_map(|items| items.iter().cloned()).collect();
            let field = Arc::new(ArrowField::new("item", item.to_arrow(), true));
            Arc::new(ListArray::try_new(field, offsets(&lists), build_array(item, &flat)?, validity(&lists))?)
        }
        DataType::Map(key, value) => {
            let maps = vals.iter().map(|v| as_entries(v, dtype)).collect::<Result<Vec<_>>>()?;
            let flat = || maps.iter().flatten().flat_map(|entries| entries.iter());
            let keys: Vec<Value> = flat().map(|(k, _)| k.clone()).collect();
            let values: Vec<Value> = flat().map(|(_, v)| v.clone()).collect();
            let ArrowType::Map(entries_field, sorted) = dtype.to_arrow() else { unreachable!("a map is an arrow map") };
            let ArrowType::Struct(kv) = entries_field.data_type().clone() else { unreachable!("entries are a struct") };
            let entries = StructArray::try_new(kv, vec![build_array(key, &keys)?, build_array(value, &values)?], None)?;
            Arc::new(MapArray::try_new(entries_field, offsets(&maps), entries, validity(&maps), sorted)?)
        }
        DataType::Struct(fields) => {
            let rows = vals.iter().map(|v| as_fields(v, fields, dtype)).collect::<Result<Vec<_>>>()?;
            let columns = fields
                .iter()
                .enumerate()
                .map(|(i, (_, t))| {
                    let column: Vec<Value> =
                        rows.iter().map(|r| r.as_ref().map_or(Value::Null, |r| r[i].clone())).collect();
                    build_array(t, &column)
                })
                .collect::<Result<Vec<_>>>()?;
            let ArrowType::Struct(arrow_fields) = dtype.to_arrow() else { unreachable!("a struct is an arrow struct") };
            let nulls = NullBuffer::from_iter(rows.iter().map(Option::is_some));
            if fields.is_empty() {
                Arc::new(StructArray::new_empty_fields(vals.len(), Some(nulls)))
            } else {
                Arc::new(StructArray::try_new(arrow_fields, columns, Some(nulls))?)
            }
        }
    })
}

fn mismatch<T>(v: &Value, dtype: &DataType) -> Result<T> {
    Err(Error::Other(format!("can't convert {v:?} to {dtype}")))
}

/// The elements of an array value; `None` for NULL.
fn as_items(v: &Value, dtype: &DataType) -> Result<Option<Vec<Value>>> {
    match v {
        Value::Null => Ok(None),
        Value::Array(items) => Ok(Some(items.clone())),
        other => mismatch(other, dtype),
    }
}

/// The entries of a map value; `None` for NULL. Engines without a map type
/// hand maps back as arrays of (key, value) structs.
fn as_entries(v: &Value, dtype: &DataType) -> Result<Option<Vec<(Value, Value)>>> {
    match v {
        Value::Null => Ok(None),
        Value::Map(entries) => Ok(Some(entries.clone())),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Struct(kv) if kv.len() == 2 => Ok((kv[0].1.clone(), kv[1].1.clone())),
                Value::Array(kv) if kv.len() == 2 => Ok((kv[0].clone(), kv[1].clone())),
                _ => mismatch(v, dtype),
            })
            .collect::<Result<Vec<_>>>()
            .map(Some),
        other => mismatch(other, dtype),
    }
}

/// The field values of a struct value in the order of `fields`; `None` for
/// NULL. Fields are matched by name, or by position when the engine returned
/// them unnamed (an array) or under names of its own.
fn as_fields(v: &Value, fields: &[(String, DataType)], dtype: &DataType) -> Result<Option<Vec<Value>>> {
    match v {
        Value::Null => Ok(None),
        Value::Struct(got) if got.len() == fields.len() => Ok(Some(
            fields
                .iter()
                .enumerate()
                .map(|(i, (name, _))| got.iter().find(|(n, _)| n == name).unwrap_or(&got[i]).1.clone())
                .collect(),
        )),
        Value::Array(got) if got.len() == fields.len() => Ok(Some(got.clone())),
        other => mismatch(other, dtype),
    }
}

fn offsets<T>(lists: &[Option<Vec<T>>]) -> OffsetBuffer<i32> {
    OffsetBuffer::from_lengths(lists.iter().map(|l| l.as_ref().map_or(0, Vec::len)))
}

fn validity<T>(lists: &[Option<Vec<T>>]) -> Option<NullBuffer> {
    Some(NullBuffer::from_iter(lists.iter().map(Option::is_some)))
}

/// Rows of values -> a batch with exactly `schema`.
pub fn rows_to_batch(schema: &Schema, rows: &[Vec<Value>]) -> Result<RecordBatch> {
    let cols = schema
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let vals: Vec<Value> = rows.iter().map(|r| r.get(i).cloned().unwrap_or(Value::Null)).collect();
            build_array(&f.dtype, &vals)
        })
        .collect::<Result<Vec<_>>>()?;
    let opts = RecordBatchOptions::new().with_row_count(Some(rows.len()));
    Ok(RecordBatch::try_new_with_options(Arc::new(schema.to_arrow()), cols, &opts)?)
}

/// Read one cell of any supported Arrow array as a `Value`.
pub fn array_value(a: &dyn Array, i: usize) -> Result<Value> {
    if a.is_null(i) {
        return Ok(Value::Null);
    }
    Ok(match a.data_type() {
        ArrowType::Null => Value::Null,
        ArrowType::Boolean => Value::Bool(a.as_boolean().value(i)),
        ArrowType::Int8 => Value::Int(a.as_primitive::<Int8Type>().value(i) as i64),
        ArrowType::Int16 => Value::Int(a.as_primitive::<Int16Type>().value(i) as i64),
        ArrowType::Int32 => Value::Int(a.as_primitive::<Int32Type>().value(i) as i64),
        ArrowType::Int64 => Value::Int(a.as_primitive::<Int64Type>().value(i)),
        ArrowType::UInt8 => Value::Int(a.as_primitive::<UInt8Type>().value(i) as i64),
        ArrowType::UInt16 => Value::Int(a.as_primitive::<UInt16Type>().value(i) as i64),
        ArrowType::UInt32 => Value::Int(a.as_primitive::<UInt32Type>().value(i) as i64),
        ArrowType::UInt64 => Value::Int(a.as_primitive::<UInt64Type>().value(i) as i64),
        ArrowType::Float32 => Value::Float(a.as_primitive::<Float32Type>().value(i) as f64),
        ArrowType::Float64 => Value::Float(a.as_primitive::<Float64Type>().value(i)),
        ArrowType::Decimal128(_, s) => {
            Value::Float(a.as_primitive::<Decimal128Type>().value(i) as f64 / 10f64.powi(*s as i32))
        }
        ArrowType::Utf8 => Value::String(a.as_string::<i32>().value(i).to_string()),
        ArrowType::LargeUtf8 => Value::String(a.as_string::<i64>().value(i).to_string()),
        ArrowType::Utf8View => Value::String(a.as_string_view().value(i).to_string()),
        ArrowType::Binary => Value::Binary(a.as_binary::<i32>().value(i).to_vec()),
        ArrowType::LargeBinary => Value::Binary(a.as_binary::<i64>().value(i).to_vec()),
        ArrowType::Date32 => Value::Date(a.as_primitive::<Date32Type>().value(i)),
        ArrowType::Date64 => Value::Date((a.as_primitive::<Date64Type>().value(i) / 86_400_000) as i32),
        ArrowType::Timestamp(unit, _) => Value::Timestamp(match unit {
            TimeUnit::Second => a.as_primitive::<TimestampSecondType>().value(i) * 1_000_000,
            TimeUnit::Millisecond => a.as_primitive::<TimestampMillisecondType>().value(i) * 1_000,
            TimeUnit::Microsecond => a.as_primitive::<TimestampMicrosecondType>().value(i),
            TimeUnit::Nanosecond => a.as_primitive::<TimestampNanosecondType>().value(i) / 1_000,
        }),
        ArrowType::List(_) => Value::Array(array_values(&a.as_list::<i32>().value(i))?),
        ArrowType::LargeList(_) => Value::Array(array_values(&a.as_list::<i64>().value(i))?),
        ArrowType::FixedSizeList(..) => Value::Array(array_values(&a.as_fixed_size_list().value(i))?),
        ArrowType::Struct(fields) => {
            let s = a.as_struct();
            Value::Struct(
                fields
                    .iter()
                    .zip(s.columns())
                    .map(|(f, c)| Ok((f.name().clone(), array_value(c.as_ref(), i)?)))
                    .collect::<Result<_>>()?,
            )
        }
        ArrowType::Map(..) => {
            let entries = a.as_map().value(i);
            let (keys, values) = (array_values(entries.column(0))?, array_values(entries.column(1))?);
            Value::Map(keys.into_iter().zip(values).collect())
        }
        other => return Err(Error::Other(format!("unsupported arrow type {other}"))),
    })
}

fn array_values(a: &dyn Array) -> Result<Vec<Value>> {
    (0..a.len()).map(|i| array_value(a, i)).collect()
}

/// A batch -> rows of values.
pub fn batch_to_rows(b: &RecordBatch) -> Result<Vec<Vec<Value>>> {
    (0..b.num_rows()).map(|i| b.columns().iter().map(|c| array_value(c.as_ref(), i)).collect()).collect()
}

/// Re-shape a batch to exactly `schema` (names and types), converting each
/// column through `Value`s where the Arrow type differs. Backends call this on
/// native results (e.g. DuckDB's HUGEINT sums come back as Decimal128(38,0)).
pub fn conform_batch(b: &RecordBatch, schema: &Schema) -> Result<RecordBatch> {
    if b.num_columns() != schema.len() {
        return Err(Error::Other(format!(
            "result has {} columns, expected {} ({schema})",
            b.num_columns(),
            schema.len()
        )));
    }
    let target = Arc::new(schema.to_arrow());
    let cols = b
        .columns()
        .iter()
        .zip(&schema.fields)
        .map(|(c, f)| {
            if c.data_type() == &f.dtype.to_arrow() {
                return Ok(c.clone());
            }
            let vals = (0..c.len()).map(|i| array_value(c.as_ref(), i)).collect::<Result<Vec<_>>>()?;
            build_array(&f.dtype, &vals)
        })
        .collect::<Result<Vec<_>>>()?;
    let opts = RecordBatchOptions::new().with_row_count(Some(b.num_rows()));
    Ok(RecordBatch::try_new_with_options(target, cols, &opts)?)
}

/// `data` shaped like the table it is going into: its columns matched to
/// `schema`'s by name (by position if the names differ) and cast to the
/// table's types.
pub fn conform_to(data: &[RecordBatch], schema: &Schema) -> Result<Vec<RecordBatch>> {
    data.iter()
        .map(|b| {
            if b.num_columns() != schema.len() {
                return Err(Error::Invalid(format!(
                    "data has {} columns, the table has {} ({schema})",
                    b.num_columns(),
                    schema.len()
                )));
            }
            let by_name: Option<Vec<usize>> = schema.names().map(|n| b.schema().index_of(n).ok()).collect();
            match by_name {
                Some(order) => conform_batch(&b.project(&order)?, schema),
                None => conform_batch(b, schema),
            }
        })
        .collect()
}
