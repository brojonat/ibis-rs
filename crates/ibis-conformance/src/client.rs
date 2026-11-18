// SPDX-License-Identifier: Apache-2.0
// Adapted from Ibis (https://github.com/ibis-project/ibis), Copyright 2015 Ibis developers.
// Modified for ibis-rs; see NOTICE at the repository root.

//! Ibis's tests of the connection itself: creating, filling, renaming and
//! dropping tables and views (ibis/backends/tests/test_client.py, and the
//! tests of that kind in test_struct.py and test_map.py).
//!
//! Unlike the expression cases these have no golden result: each is a
//! procedure run against the backend, which passes if every check in it
//! holds. Ids are `client::<ibis module>::<pytest id>`.

use std::sync::atomic::{AtomicUsize, Ordering};

use ibis_core::convert::{batch_to_rows, rows_to_batch};
use ibis_core::prelude::*;
use ibis_core::{Error, Result, Value};
use serde_json::json;

use crate::cases::NO_NESTED;
use crate::{Outcome, PARTIAL, Status, Target};

type Run = Box<dyn Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync>;

pub struct ClientCase {
    /// The Ibis test module the test is in.
    pub module: &'static str,
    /// The pytest id within the module.
    pub id: String,
    run: Run,
    pub notimpl: Vec<&'static str>,
}

impl ClientCase {
    pub fn full_id(&self) -> String {
        format!("client::{}::{}", self.module, self.id)
    }

    fn notimpl(mut self, backends: &[&'static str]) -> Self {
        self.notimpl.extend(backends);
        self
    }
}

fn case(id: &str, run: impl Fn(&dyn Backend, &Target) -> Result<()> + Send + Sync + 'static) -> ClientCase {
    ClientCase { module: "test_client", id: id.into(), run: Box::new(run), notimpl: vec![] }
}

/// A check that did not hold: reported as a wrong answer, not as an error.
fn failed(what: impl Into<String>) -> Error {
    Error::Other(format!("check failed: {}", what.into()))
}

fn check(holds: bool, what: impl FnOnce() -> String) -> Result<()> {
    if holds { Ok(()) } else { Err(failed(what())) }
}

/// A table name no other test uses, and the table dropped when this goes.
struct Temp<'a> {
    backend: &'a dyn Backend,
    name: String,
    view: bool,
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

impl<'a> Temp<'a> {
    fn table(backend: &'a dyn Backend) -> Temp<'a> {
        let name = format!("ibis_client_{}", NEXT.fetch_add(1, Ordering::Relaxed));
        Temp { backend, name, view: false }
    }

    fn view(backend: &'a dyn Backend) -> Temp<'a> {
        let mut temp = Temp::table(backend);
        temp.view = true;
        temp
    }
}

impl Drop for Temp<'_> {
    fn drop(&mut self) {
        // best effort: the test's own result is what matters
        let _ = match self.view {
            true => self.backend.drop_view(&self.name, true),
            false => self.backend.drop_table(&self.name, true),
        };
    }
}

fn strings() -> DataType {
    DataType::String
}

fn employee_schema() -> Schema {
    Schema::of(&[
        ("first_name", strings()),
        ("last_name", strings()),
        ("department_name", strings()),
        ("salary", DataType::Float64),
    ])
}

type Employee = (&'static str, &'static str, &'static str, f64);

const EMPLOYEES_1: [Employee; 3] = [("A", "D", "AA", 100.0), ("B", "E", "BB", 200.0), ("C", "F", "CC", 300.0)];
const EMPLOYEES_2: [Employee; 3] = [("X", "A", "XX", 400.0), ("Y", "B", "YY", 500.0), ("Z", "C", "ZZ", 600.0)];

fn employees(rows: &[Employee]) -> Vec<Vec<Value>> {
    rows.iter().map(|(a, b, c, d)| vec![(*a).into(), (*b).into(), (*c).into(), (*d).into()]).collect()
}

fn create(b: &dyn Backend, name: &str, schema: &Schema, rows: &[Vec<Value>]) -> Result<()> {
    b.create_table(name, &[rows_to_batch(schema, rows)?])
}

/// The rows of a table, sorted by its first column.
fn rows_of(b: &dyn Backend, name: &str) -> Result<Vec<Vec<Value>>> {
    let t = b.table(name)?;
    let first = t.schema().fields[0].name.clone();
    let mut rows = vec![];
    for batch in b.execute(&t.order_by([first.as_str()])?)? {
        rows.extend(batch_to_rows(&batch)?);
    }
    Ok(rows)
}

fn check_rows(b: &dyn Backend, name: &str, want: &[Vec<Value>]) -> Result<()> {
    let got = rows_of(b, name)?;
    check(got == want, || format!("table {name}: want {want:?}, got {got:?}"))
}

/// Column names and types as text, for comparing and for messages.
fn described(schema: &Schema) -> Vec<(String, String)> {
    schema.fields.iter().map(|f| (f.name.clone(), f.dtype.to_string())).collect()
}

pub fn all() -> Vec<ClientCase> {
    let mut v = vec![];

    // Ibis passes the same rows as a DataFrame or a PyArrow table, with the
    // schema given in four ways; here the data is always Arrow with a schema.
    for sch in ["no_schema", "dict_schema", "tuples", "schema"] {
        for func in ["dataframe", "pyarrow_table"] {
            v.push(case(&format!("test_create_table[{sch}-{func}]"), |b, _| {
                let t = Temp::table(b);
                let schema = Schema::of(&[("first_name", strings()), ("salary", DataType::Float64)]);
                let rows: Vec<Vec<Value>> = [("A", 100.0), ("B", 200.0), ("C", 300.0)]
                    .iter()
                    .map(|(n, s)| vec![(*n).into(), (*s).into()])
                    .collect();
                create(b, &t.name, &schema, &rows)?;
                check_rows(b, &t.name, &rows)
            }));
        }
    }
    for how in ["dataframe", "pyarrow table"] {
        v.push(case(&format!("test_load_data[{how}]"), |b, _| {
            let t = Temp::table(b);
            let rows = employees(&EMPLOYEES_1);
            // twice: the second replaces the first
            create(b, &t.name, &employee_schema(), &rows)?;
            create(b, &t.name, &employee_schema(), &rows)?;
            check_rows(b, &t.name, &rows)
        }));
    }
    v.push(case("test_query_schema[column]", |_, target| {
        let got = described(target.fx.alltypes.select(["string_col"])?.schema());
        check(got == [("string_col".to_string(), "string".to_string())], || format!("{got:?}"))
    }));
    v.push(case("test_query_schema[table]", |_, target| {
        let got = described(target.fx.alltypes.select(["string_col", "bigint_col"])?.schema());
        let want = [("string_col".to_string(), "string".to_string()), ("bigint_col".to_string(), "int64".to_string())];
        check(got == want, || format!("{got:?}"))
    }));

    let abc = || Schema::of(&[("a", strings()), ("b", DataType::Boolean), ("c", DataType::Int32)]);
    v.push(case("test_create_table_from_schema", move |b, _| {
        let t = Temp::table(b);
        b.create_empty_table(&t.name, &abc())?;
        let got = described(b.table(&t.name)?.schema());
        check(got == described(&abc()), || format!("want {:?}, got {got:?}", described(&abc())))?;
        check_rows(b, &t.name, &[])
    }));
    v.push(case("test_rename_table", move |b, _| {
        let (old, new) = (Temp::table(b), Temp::table(b));
        b.create_empty_table(&old.name, &abc())?;
        b.rename_table(&old.name, &new.name)?;
        let got = described(b.table(&new.name)?.schema());
        check(got == described(&abc()), || format!("want {:?}, got {got:?}", described(&abc())))?;
        let tables = b.list_tables()?;
        check(!tables.contains(&old.name), || format!("{} is still listed", old.name))
    }));
    v.push(case("test_create_drop_view", |b, target| {
        let view = Temp::view(b);
        b.create_view(&view.name, &target.fx.alltypes.limit(1, 0), false)?;
        check(b.list_tables()?.contains(&view.name), || format!("{} is not listed", view.name))?;
        let names = |t: &Table| -> Vec<String> {
            let mut names: Vec<String> = t.schema().names().map(|n| n.to_lowercase()).collect();
            names.sort();
            names
        };
        let (want, got) = (names(&target.fx.alltypes), names(&b.table(&view.name)?));
        check(got == want, || format!("the view's columns are {got:?}, the table's {want:?}"))?;
        b.drop_view(&view.name, false)?;
        check(!b.list_tables()?.contains(&view.name), || format!("{} is still listed", view.name))
    }));

    v.push(case("test_insert_no_overwrite_from_dataframe", |b, _| {
        let t = Temp::table(b);
        b.create_empty_table(&t.name, &employee_schema())?;
        b.insert(&t.name, &[rows_to_batch(&employee_schema(), &employees(&EMPLOYEES_2))?], false)?;
        check_rows(b, &t.name, &employees(&EMPLOYEES_2))
    }));
    v.push(case("test_insert_overwrite_from_dataframe", |b, _| {
        let t = Temp::table(b);
        create(b, &t.name, &employee_schema(), &employees(&EMPLOYEES_1))?;
        b.insert(&t.name, &[rows_to_batch(&employee_schema(), &employees(&EMPLOYEES_2))?], true)?;
        check_rows(b, &t.name, &employees(&EMPLOYEES_2))
    }));
    v.push(case("test_insert_no_overwrite_from_expr", |b, _| {
        let (t, from) = (Temp::table(b), Temp::table(b));
        b.create_empty_table(&t.name, &employee_schema())?;
        create(b, &from.name, &employee_schema(), &employees(&EMPLOYEES_2))?;
        b.insert_from(&t.name, &b.table(&from.name)?, false)?;
        check_rows(b, &t.name, &employees(&EMPLOYEES_2))
    }));
    v.push(case("test_insert_overwrite_from_expr", |b, _| {
        let (t, from) = (Temp::table(b), Temp::table(b));
        create(b, &t.name, &employee_schema(), &employees(&EMPLOYEES_1))?;
        create(b, &from.name, &employee_schema(), &employees(&EMPLOYEES_2))?;
        b.insert_from(&t.name, &b.table(&from.name)?, true)?;
        check_rows(b, &t.name, &employees(&EMPLOYEES_2))
    }));
    v.push(case("test_insert_overwrite_from_list", |b, _| {
        let t = Temp::table(b);
        create(b, &t.name, &employee_schema(), &employees(&EMPLOYEES_1))?;
        // a salary given as an integer, as in Ibis's list of dicts
        let rows = vec![
            vec!["Adam".into(), "Smith".into(), "Accounting".into(), 50000.0.into()],
            vec!["Mohammed".into(), "Ali".into(), "Boxing".into(), Value::Int(150000)],
            vec!["María".into(), "Gonzalez".into(), "Engineering".into(), 100000.0.into()],
        ];
        b.insert(&t.name, &[rows_to_batch(&employee_schema(), &rows)?], true)?;
        let got = rows_of(b, &t.name)?;
        check(got.len() == 3, || format!("want 3 rows, got {got:?}"))
    }));
    v.push(case("test_unsigned_integer_type", |b, _| {
        let t = Temp::table(b);
        let schema = Schema::of(&[
            ("a", DataType::UInt8),
            ("b", DataType::UInt16),
            ("c", DataType::UInt32),
            ("d", DataType::UInt64),
        ]);
        b.create_empty_table(&t.name, &schema)?;
        check(b.list_tables()?.contains(&t.name), || format!("{} is not listed", t.name))
    }));
    v.push(case("test_table_not_found", |b, _| {
        let missing = Temp::table(b);
        check(b.table(&missing.name).is_err(), || format!("the table {} is not there, yet was found", missing.name))
    }));

    // field names that are SQL keywords
    for id in ["nullable", "non-nullable"] {
        let run = |b: &dyn Backend, _: &Target| {
            let t = Temp::table(b);
            let fields = ["select", "from", "where", "order", "left join", "full outer join"];
            let keywords = DataType::Struct(fields.iter().map(|f| (f.to_string(), DataType::Int64)).collect());
            let schema = Schema::of(&[("struct", keywords)]);
            b.create_empty_table(&t.name, &schema)?;
            let got = described(b.table(&t.name)?.schema());
            check(got == described(&schema), || format!("want {:?}, got {got:?}", described(&schema)))?;
            check_rows(b, &t.name, &[])
        };
        let mut c = case(&format!("test_keyword_fields[{id}]"), run).notimpl(&NO_NESTED);
        c.module = "test_struct";
        v.push(c);
    }
    let mut c = case("test_map_create_table", |b, _| {
        let t = Temp::table(b);
        let schema = Schema::of(&[("xyz", DataType::map(strings(), strings()))]);
        b.create_empty_table(&t.name, &schema)?;
        let got = b.table(&t.name)?.schema().fields[0].dtype.clone();
        check(matches!(got, DataType::Map(..)), || format!("xyz is {got}, not a map"))
    })
    .notimpl(&NO_NESTED);
    c.module = "test_map";
    v.push(c);
    v
}

/// Run every client test whose id contains `filter` on one backend.
pub fn run(target: &Target, filter: &str) -> Vec<Outcome> {
    let backend = target.backend.as_ref();
    all()
        .into_iter()
        .filter(|c| c.full_id().contains(filter))
        .map(|c| {
            let (mut status, mut detail) = (Status::Pass, json!(null));
            if let Err(e) = (c.run)(backend, target) {
                let message = e.to_string();
                status = match &e {
                    Error::Unsupported { .. } if PARTIAL.contains(&backend.name()) => Status::Unsupported,
                    _ if message.starts_with("check failed: ") => Status::Fail,
                    _ => Status::Error,
                };
                detail = json!(message);
            }
            if c.notimpl.contains(&backend.name()) {
                status = if status == Status::Pass { Status::Xpass } else { Status::Xfail };
            }
            Outcome { id: c.full_id(), backend: backend.name().into(), status, sql: None, detail }
        })
        .collect()
}
