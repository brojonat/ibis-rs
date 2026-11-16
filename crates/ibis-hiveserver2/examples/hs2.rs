//! Run statements on a HiveServer2 endpoint and print each result as
//! tab-separated text, types on the first line:
//!
//!     cargo run -p ibis-hiveserver2 --example hs2 -- hive2://me@localhost:10000 'SELECT 1'
//!     cargo run -p ibis-hiveserver2 --example hs2 -- 'hive2://localhost:21050/db?auth=nosasl' 'SELECT 1' 'SELECT 2'

use ibis_hiveserver2::{Connection, Options, Values};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [url, statements @ ..] = args.as_slice() else {
        eprintln!("usage: hs2 URL STATEMENT...");
        std::process::exit(2);
    };
    let options = Options::from_url(url, 10000, true).unwrap_or_else(|e| {
        eprintln!("hs2: {e}");
        std::process::exit(2);
    });
    let mut con = Connection::open(&options).unwrap_or_else(|e| {
        eprintln!("hs2: {e}");
        std::process::exit(1);
    });
    for sql in statements {
        let start = std::time::Instant::now();
        match con.execute(sql) {
            Err(e) => {
                eprintln!("hs2: {e}");
                std::process::exit(1);
            }
            Ok(result) => {
                let header: Vec<String> =
                    result.columns.iter().map(|c| format!("{}:{}", c.name, c.type_name)).collect();
                println!("{}", header.join("\t"));
                for row in 0..result.num_rows() {
                    let cells: Vec<String> = result
                        .columns
                        .iter()
                        .map(|c| match &c.values {
                            Values::Bool(v) => v[row].map_or("NULL".into(), |x| x.to_string()),
                            Values::Int(v) => v[row].map_or("NULL".into(), |x| x.to_string()),
                            Values::Double(v) => v[row].map_or("NULL".into(), |x| format!("{x:?}")),
                            Values::String(v) => v[row].clone().unwrap_or("NULL".into()),
                            Values::Binary(v) => v[row].as_ref().map_or("NULL".into(), |x| format!("{x:02x?}")),
                        })
                        .collect();
                    println!("{}", cells.join("\t"));
                }
                eprintln!("hs2: {} rows in {:?}", result.num_rows(), start.elapsed());
            }
        }
    }
}
