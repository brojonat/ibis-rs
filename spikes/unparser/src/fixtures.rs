//! Deterministic fixture data, rendered as DDL + INSERTs for each engine.
//!
//! The data deliberately includes the things that break portability:
//! NULLs, negative numbers (integer division / modulo / cast rounding),
//! mixed-case and padded strings, foreign keys with no match (left joins),
//! and timestamps spanning months and years.

#[derive(Clone, Debug)]
pub enum Val {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Date(String),
    Ts(String),
    Null,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Engine {
    DataFusion,
    Postgres,
    DuckDb,
    Sqlite,
}

#[derive(Clone, Copy)]
enum Ty {
    Int,
    Float,
    Str,
    Bool,
    Date,
    Ts,
}

pub struct Table {
    pub name: &'static str,
    cols: Vec<(&'static str, Ty)>,
    rows: Vec<Vec<Val>>,
}

fn ty_sql(ty: Ty, e: Engine) -> &'static str {
    use Engine::*;
    match (ty, e) {
        (Ty::Int, Sqlite) => "INTEGER",
        (Ty::Int, _) => "BIGINT",
        (Ty::Float, Sqlite) => "REAL",
        (Ty::Float, Postgres) => "DOUBLE PRECISION",
        (Ty::Float, _) => "DOUBLE",
        (Ty::Str, Sqlite) => "TEXT",
        (Ty::Str, _) => "VARCHAR",
        (Ty::Bool, Sqlite) => "INTEGER",
        (Ty::Bool, _) => "BOOLEAN",
        (Ty::Date, Sqlite) => "TEXT",
        (Ty::Date, _) => "DATE",
        (Ty::Ts, Sqlite) => "TEXT",
        (Ty::Ts, _) => "TIMESTAMP",
    }
}

fn val_sql(v: &Val, e: Engine) -> String {
    match v {
        Val::Int(i) => i.to_string(),
        Val::Float(f) => format!("{f:?}"),
        Val::Str(s) => format!("'{}'", s.replace('\'', "''")),
        Val::Bool(b) if e == Engine::Sqlite => (*b as i32).to_string(),
        Val::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Val::Date(d) if e == Engine::Sqlite => format!("'{d}'"),
        Val::Date(d) => format!("DATE '{d}'"),
        Val::Ts(t) if e == Engine::Sqlite => format!("'{t}'"),
        Val::Ts(t) => format!("TIMESTAMP '{t}'"),
        Val::Null => "NULL".into(),
    }
}

impl Table {
    /// Statements that (re)create and populate this table on `e`.
    pub fn statements(&self, e: Engine) -> Vec<String> {
        let cols: Vec<String> = self
            .cols
            .iter()
            .map(|(n, t)| format!("{n} {}", ty_sql(*t, e)))
            .collect();
        let rows: Vec<String> = self
            .rows
            .iter()
            .map(|r| {
                let vs: Vec<String> = r.iter().map(|v| val_sql(v, e)).collect();
                format!("({})", vs.join(", "))
            })
            .collect();
        let mut out = vec![];
        if e != Engine::DataFusion {
            out.push(format!("DROP TABLE IF EXISTS {}", self.name));
        }
        out.push(format!("CREATE TABLE {} ({})", self.name, cols.join(", ")));
        out.push(format!("INSERT INTO {} VALUES {}", self.name, rows.join(", ")));
        out
    }
}

pub fn tables() -> Vec<Table> {
    let customers = Table {
        name: "customers",
        cols: vec![
            ("id", Ty::Int),
            ("name", Ty::Str),
            ("region", Ty::Str),
            ("signup", Ty::Date),
        ],
        rows: [
            (1, "Alice", Some("west"), "2023-01-15"),
            (2, "bob", Some("east"), "2023-02-28"),
            (3, "Carol Ann", None, "2023-03-01"),
            (4, "  dave  ", Some("west"), "2023-12-31"),
            (5, "Eve", Some("south"), "2024-02-29"),
            (6, "Frank", Some("east"), "2024-06-10"),
            (7, "Grace", Some("west"), "2024-07-04"),
            (8, "Heidi", None, "2024-11-11"),
        ]
        .into_iter()
        .map(|(id, name, region, signup)| {
            vec![
                Val::Int(id),
                Val::Str(name.into()),
                region.map_or(Val::Null, |r| Val::Str(r.into())),
                Val::Date(signup.into()),
            ]
        })
        .collect(),
    };

    // Small LCG so the data is deterministic without a rand dependency.
    let mut seed: u64 = 42;
    let mut next = move || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        seed >> 33
    };
    let statuses = ["shipped", "pending", "returned", "Shipped"];
    let rows = (1..=40i64)
        .map(|i| {
            let r = next();
            let customer = if i % 13 == 0 { Val::Null } else { Val::Int((i * 7) % 9 + 1) };
            let amount = if i % 11 == 0 {
                Val::Null
            } else {
                Val::Float((r % 20000) as f64 / 100.0 - 20.0)
            };
            let quantity = (next() % 11) as i64 - 3;
            let ts = format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:00",
                2023 + i / 30,
                (i % 12) + 1,
                (i * 7 % 28) + 1,
                i % 24,
                (i * 13) % 60
            );
            vec![
                Val::Int(i),
                customer,
                amount,
                Val::Int(quantity),
                Val::Str(statuses[(i % 4) as usize].into()),
                Val::Ts(ts),
                Val::Bool(i % 5 == 0),
            ]
        })
        .collect();
    let orders = Table {
        name: "orders",
        cols: vec![
            ("id", Ty::Int),
            ("customer_id", Ty::Int),
            ("amount", Ty::Float),
            ("quantity", Ty::Int),
            ("status", Ty::Str),
            ("created_at", Ty::Ts),
            ("is_gift", Ty::Bool),
        ],
        rows,
    };
    vec![customers, orders]
}
