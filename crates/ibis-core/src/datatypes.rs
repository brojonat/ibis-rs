//! Ibis datatypes and schemas, and their mapping to Arrow.

use std::fmt;

use arrow_schema::{DataType as ArrowType, Field as ArrowField, Schema as ArrowSchema, TimeUnit};

use crate::error::{Result, invalid};

/// Logical types, following Ibis's dtype names (`int32`, `float64`, `string`, ...).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DataType {
    /// The type of an untyped NULL literal; unifies with anything.
    Null,
    Boolean,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Decimal {
        precision: u8,
        scale: i8,
    },
    String,
    Binary,
    Date,
    /// Timestamp without (or with) a time zone. Values are microseconds.
    Timestamp {
        tz: Option<String>,
    },
}

impl DataType {
    pub fn is_integer(&self) -> bool {
        use DataType::*;
        matches!(self, Int8 | Int16 | Int32 | Int64 | UInt8 | UInt16 | UInt32 | UInt64)
    }
    pub fn is_floating(&self) -> bool {
        matches!(self, DataType::Float32 | DataType::Float64)
    }
    pub fn is_numeric(&self) -> bool {
        self.is_integer() || self.is_floating() || matches!(self, DataType::Decimal { .. })
    }
    pub fn is_temporal(&self) -> bool {
        matches!(self, DataType::Date | DataType::Timestamp { .. })
    }

    fn int_width(&self) -> u8 {
        use DataType::*;
        match self {
            Int8 | UInt8 => 8,
            Int16 | UInt16 => 16,
            Int32 | UInt32 => 32,
            _ => 64,
        }
    }

    /// The type both operands of an arithmetic op are promoted to.
    pub fn promote(a: &DataType, b: &DataType) -> Result<DataType> {
        use DataType::*;
        Ok(match (a, b) {
            (Null, x) | (x, Null) => x.clone(),
            (x, y) if x == y => x.clone(),
            (Float32, Float32) => Float32,
            (x, y) if x.is_floating() || y.is_floating() => Float64,
            (Decimal { .. }, y) | (y, Decimal { .. }) if y.is_integer() => a_or_decimal(a, b),
            (x, y) if x.is_integer() && y.is_integer() => match x.int_width().max(y.int_width()) {
                8 => Int8,
                16 => Int16,
                32 => Int32,
                _ => Int64,
            },
            (x, y) if x.is_numeric() && y.is_numeric() => Float64,
            (x, y) => return invalid(format!("cannot combine {x} and {y}")),
        })
    }

    pub fn to_arrow(&self) -> ArrowType {
        use DataType::*;
        match self {
            Null => ArrowType::Null,
            Boolean => ArrowType::Boolean,
            Int8 => ArrowType::Int8,
            Int16 => ArrowType::Int16,
            Int32 => ArrowType::Int32,
            Int64 => ArrowType::Int64,
            UInt8 => ArrowType::UInt8,
            UInt16 => ArrowType::UInt16,
            UInt32 => ArrowType::UInt32,
            UInt64 => ArrowType::UInt64,
            Float32 => ArrowType::Float32,
            Float64 => ArrowType::Float64,
            Decimal { precision, scale } => ArrowType::Decimal128(*precision, *scale),
            String => ArrowType::Utf8,
            Binary => ArrowType::Binary,
            Date => ArrowType::Date32,
            Timestamp { tz } => ArrowType::Timestamp(TimeUnit::Microsecond, tz.as_deref().map(Into::into)),
        }
    }

    pub fn from_arrow(t: &ArrowType) -> Result<DataType> {
        use DataType::*;
        Ok(match t {
            ArrowType::Null => Null,
            ArrowType::Boolean => Boolean,
            ArrowType::Int8 => Int8,
            ArrowType::Int16 => Int16,
            ArrowType::Int32 => Int32,
            ArrowType::Int64 => Int64,
            ArrowType::UInt8 => UInt8,
            ArrowType::UInt16 => UInt16,
            ArrowType::UInt32 => UInt32,
            ArrowType::UInt64 => UInt64,
            ArrowType::Float16 | ArrowType::Float32 => Float32,
            ArrowType::Float64 => Float64,
            ArrowType::Decimal128(p, s) => Decimal { precision: *p, scale: *s },
            ArrowType::Utf8 | ArrowType::LargeUtf8 | ArrowType::Utf8View => String,
            ArrowType::Binary | ArrowType::LargeBinary | ArrowType::BinaryView => Binary,
            ArrowType::Date32 | ArrowType::Date64 => Date,
            ArrowType::Timestamp(_, tz) => Timestamp { tz: tz.as_ref().map(|s| s.to_string()) },
            other => return invalid(format!("unsupported arrow type {other}")),
        })
    }
}

fn a_or_decimal(a: &DataType, b: &DataType) -> DataType {
    if matches!(a, DataType::Decimal { .. }) { a.clone() } else { b.clone() }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use DataType::*;
        let s = match self {
            Null => "null",
            Boolean => "boolean",
            Int8 => "int8",
            Int16 => "int16",
            Int32 => "int32",
            Int64 => "int64",
            UInt8 => "uint8",
            UInt16 => "uint16",
            UInt32 => "uint32",
            UInt64 => "uint64",
            Float32 => "float32",
            Float64 => "float64",
            Decimal { precision, scale } => return write!(f, "decimal({precision}, {scale})"),
            String => "string",
            Binary => "binary",
            Date => "date",
            Timestamp { tz: None } => "timestamp",
            Timestamp { tz: Some(tz) } => return write!(f, "timestamp('{tz}')"),
        };
        f.write_str(s)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub dtype: DataType,
    pub nullable: bool,
}

impl Field {
    pub fn new(name: impl Into<String>, dtype: DataType) -> Self {
        Field { name: name.into(), dtype, nullable: true }
    }
}

/// An ordered list of named, typed columns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Schema {
    pub fields: Vec<Field>,
}

impl Schema {
    pub fn new(fields: Vec<Field>) -> Self {
        Schema { fields }
    }

    /// `Schema::of(&[("id", DataType::Int64), ...])`
    pub fn of(cols: &[(&str, DataType)]) -> Self {
        Schema::new(cols.iter().map(|(n, t)| Field::new(*n, t.clone())).collect())
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(|f| f.name.as_str())
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn to_arrow(&self) -> ArrowSchema {
        ArrowSchema::new(
            self.fields.iter().map(|f| ArrowField::new(&f.name, f.dtype.to_arrow(), f.nullable)).collect::<Vec<_>>(),
        )
    }

    pub fn from_arrow(s: &ArrowSchema) -> Result<Schema> {
        let fields = s
            .fields()
            .iter()
            .map(|f| {
                Ok(Field {
                    name: f.name().clone(),
                    dtype: DataType::from_arrow(f.data_type())?,
                    nullable: f.is_nullable(),
                })
            })
            .collect::<Result<_>>()?;
        Ok(Schema { fields })
    }
}

impl fmt::Display for Schema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, fld) in self.fields.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{}: {}", fld.name, fld.dtype)?;
        }
        Ok(())
    }
}
