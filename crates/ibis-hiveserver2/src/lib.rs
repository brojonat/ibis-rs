//! A minimal HiveServer2 client.
//!
//! HiveServer2 is the Thrift service (`TCLIService`) that Hive, the Spark
//! Thrift server and Impala all answer SQL on. This crate opens a session,
//! runs a statement to completion and returns its result as columns. It
//! speaks the binary protocol over a plain socket, with or without the SASL
//! `PLAIN` handshake; no TLS, Kerberos or HTTP transport.
//!
//! ```no_run
//! use ibis_hiveserver2::{Connection, Options};
//!
//! let mut con = Connection::open(&Options::from_url("hive2://me@localhost:10000/default", 10000, true)?)?;
//! let result = con.execute("SELECT 1 AS one")?;
//! assert_eq!(result.columns[0].name, "one");
//! # Ok::<(), ibis_hiveserver2::Error>(())
//! ```

#[cfg(test)]
mod tests;
mod thrift;

use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use thrift::{Value as T, string};

/// `HIVE_CLI_SERVICE_PROTOCOL_V6`: the first with column-wise results, and
/// the newest Impala speaks.
const PROTOCOL_V6: i32 = 5;
/// Rows asked for per fetch.
const FETCH_ROWS: i64 = 10_000;

#[derive(Debug)]
pub enum Error {
    /// The connection failed or the bytes on it made no sense.
    Io(io::Error),
    /// The server refused the session or the statement.
    Server(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Server(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Where to connect and as whom.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    /// The session's current database; empty for the server's default.
    pub database: String,
    /// Do the SASL `PLAIN` handshake and frame messages: what a server with
    /// `hive.server2.authentication=NONE` (the default) or `LDAP` expects.
    /// Without it the socket is used bare, as `NOSASL` servers and Impala
    /// without authentication expect.
    pub sasl: bool,
}

impl Options {
    /// Read `scheme://[user[:password]@]host[:port][/database][?auth=plain|nosasl]`.
    /// The scheme is the caller's to check; the port and whether to use SASL
    /// default to what is passed.
    pub fn from_url(url: &str, default_port: u16, default_sasl: bool) -> Result<Options> {
        let bad = |why: &str| Error::Io(invalid(format!("{why} in {url:?}")));
        let (_, rest) = url.split_once("://").ok_or_else(|| bad("no scheme"))?;
        let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, database) = rest.split_once('/').unwrap_or((rest, ""));
        let (credentials, address) = authority.rsplit_once('@').unwrap_or(("", authority));
        let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
        let (host, port) = match address.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().map_err(|_| bad("a port that is not a number"))?),
            None => (address, default_port),
        };
        if host.is_empty() {
            return Err(bad("no host"));
        }
        let mut sasl = default_sasl;
        for pair in query.split('&').filter(|pair| !pair.is_empty()) {
            match pair.split_once('=') {
                Some(("auth", "plain")) => sasl = true,
                Some(("auth", "nosasl")) => sasl = false,
                _ => return Err(bad(&format!("unknown parameter {pair:?} (auth=plain and auth=nosasl are known)"))),
            }
        }
        Ok(Options {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            database: database.trim_end_matches('/').into(),
            sasl,
        })
    }
}

/// One result column: values of a single kind, `None` for NULL.
#[derive(Debug, Clone, PartialEq)]
pub enum Values {
    Bool(Vec<Option<bool>>),
    /// `tinyint` to `bigint`.
    Int(Vec<Option<i64>>),
    /// `float` and `double`.
    Double(Vec<Option<f64>>),
    /// Strings, and whatever else is sent as text: decimals, dates, timestamps.
    String(Vec<Option<String>>),
    Binary(Vec<Option<Vec<u8>>>),
}

impl Values {
    pub fn len(&self) -> usize {
        match self {
            Values::Bool(v) => v.len(),
            Values::Int(v) => v.len(),
            Values::Double(v) => v.len(),
            Values::String(v) => v.len(),
            Values::Binary(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn append(&mut self, more: Values) -> io::Result<()> {
        match (self, more) {
            (Values::Bool(a), Values::Bool(b)) => a.extend(b),
            (Values::Int(a), Values::Int(b)) => a.extend(b),
            (Values::Double(a), Values::Double(b)) => a.extend(b),
            (Values::String(a), Values::String(b)) => a.extend(b),
            (Values::Binary(a), Values::Binary(b)) => a.extend(b),
            _ => return Err(invalid("a column changed type between fetches")),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    /// The SQL type in lower case, as `DESCRIBE` spells it: `bigint`,
    /// `decimal(15,2)`, `timestamp`.
    pub type_name: String,
    pub values: Values,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResultSet {
    pub columns: Vec<Column>,
}

impl ResultSet {
    pub fn num_rows(&self) -> usize {
        self.columns.first().map_or(0, |c| c.values.len())
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// The socket, bare or carrying SASL frames.
struct Transport {
    stream: BufReader<TcpStream>,
    framed: bool,
    /// What is left of the frame being read.
    frame: io::Cursor<Vec<u8>>,
}

impl Transport {
    fn send(&mut self, message: &[u8]) -> io::Result<()> {
        let stream = self.stream.get_mut();
        if self.framed {
            stream.write_all(&(message.len() as u32).to_be_bytes())?;
        }
        stream.write_all(message)?;
        stream.flush()
    }

    /// One step of the SASL negotiation: a status byte and a payload.
    fn send_sasl(&mut self, status: u8, payload: &[u8]) -> io::Result<()> {
        let stream = self.stream.get_mut();
        stream.write_all(&[status])?;
        stream.write_all(&(payload.len() as u32).to_be_bytes())?;
        stream.write_all(payload)?;
        stream.flush()
    }
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.framed {
            return self.stream.read(buf);
        }
        if self.frame.position() as usize == self.frame.get_ref().len() {
            let mut len = [0; 4];
            self.stream.read_exact(&mut len)?;
            let mut frame = vec![0; u32::from_be_bytes(len) as usize];
            self.stream.read_exact(&mut frame)?;
            self.frame = io::Cursor::new(frame);
        }
        self.frame.read(buf)
    }
}

const SASL_START: u8 = 1;
const SASL_OK: u8 = 2;
const SASL_COMPLETE: u8 = 5;

/// An open session. Statements run one at a time; dropping it closes the session.
pub struct Connection {
    transport: Transport,
    session: T,
    seq: i32,
}

/// `TStatus`: an error unless the code is SUCCESS or SUCCESS_WITH_INFO.
fn check(response: &T) -> Result<()> {
    let status = response.field(1).ok_or_else(|| invalid("response without a status"))?;
    match status.field(1).and_then(T::as_i32) {
        Some(0 | 1) => Ok(()),
        // STILL_EXECUTING is only an answer to a fetch
        Some(2) => Ok(()),
        _ => Err(Error::Server(status.field(5).and_then(T::as_str).unwrap_or("the server reported an error").into())),
    }
}

/// The names `DESCRIBE` uses, by `TTypeId`.
const TYPE_NAMES: [&str; 23] = [
    "boolean",
    "tinyint",
    "smallint",
    "int",
    "bigint",
    "float",
    "double",
    "string",
    "timestamp",
    "binary",
    "array",
    "map",
    "struct",
    "union",
    "user_defined",
    "decimal",
    "null",
    "date",
    "varchar",
    "char",
    "interval_year_month",
    "interval_day_time",
    "timestamp with local time zone",
];

/// A `TColumnDesc` as (name, type name).
fn column_desc(desc: &T) -> (String, String) {
    let name = desc.field(1).and_then(T::as_str).unwrap_or("").to_string();
    // TTypeDesc.types[0].primitiveEntry
    let primitive = desc.field(2).and_then(|t| t.field(1)).and_then(|types| types.as_list().first()?.field(1));
    let Some(primitive) = primitive else { return (name, "string".into()) };
    let id = primitive.field(1).and_then(T::as_i32).unwrap_or(7);
    let base = TYPE_NAMES.get(id as usize).copied().unwrap_or("string");
    // TTypeQualifiers.qualifiers: name -> union { 1: i32, 2: string }
    let qualifier = |key: &str| {
        let qualifiers = primitive.field(2)?.field(1)?.as_map();
        qualifiers.iter().find(|(k, _)| k.as_str() == Some(key))?.1.field(1)?.as_i32()
    };
    let type_name = match (base, qualifier("precision"), qualifier("scale"), qualifier("characterMaximumLength")) {
        ("decimal", Some(p), Some(s), _) => format!("decimal({p},{s})"),
        ("varchar" | "char", _, _, Some(n)) => format!("{base}({n})"),
        _ => base.to_string(),
    };
    (name, type_name)
}

/// Whether row `i` is NULL: bit `i`, lowest bit first, of the `nulls` bytes.
fn is_null(nulls: &[u8], i: usize) -> bool {
    nulls.get(i / 8).is_some_and(|byte| byte >> (i % 8) & 1 == 1)
}

/// A `TColumn`: a union of typed columns, each a list of values and a NULL bitmap.
fn column_values(column: T) -> io::Result<Values> {
    let T::Struct(mut fields) = column else { return Err(invalid("a result column is not a struct")) };
    let Some((kind, T::Struct(parts))) = fields.pop() else { return Err(invalid("an empty result column")) };
    let (mut values, mut nulls) = (vec![], vec![]);
    for (id, part) in parts {
        match (id, part) {
            (1, T::List(_, items)) => values = items,
            (2, T::Binary(bytes)) => nulls = bytes,
            _ => {}
        }
    }
    macro_rules! collect {
        ($variant:ident, $pattern:pat => $value:expr) => {
            Values::$variant(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| match v {
                        _ if is_null(&nulls, i) => None,
                        $pattern => Some($value),
                        _ => None,
                    })
                    .collect(),
            )
        };
    }
    Ok(match kind {
        1 => collect!(Bool, T::Bool(b) => b),
        2 => collect!(Int, T::Byte(i) => i as i64),
        3 => collect!(Int, T::I16(i) => i as i64),
        4 => collect!(Int, T::I32(i) => i as i64),
        5 => collect!(Int, T::I64(i) => i),
        6 => collect!(Double, T::Double(f) => f),
        7 => collect!(String, T::Binary(b) => String::from_utf8_lossy(&b).into_owned()),
        8 => collect!(Binary, T::Binary(b) => b),
        other => return Err(invalid(format!("unknown result column kind {other}"))),
    })
}

impl Connection {
    pub fn open(options: &Options) -> Result<Connection> {
        let address = (options.host.as_str(), options.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| invalid(format!("{}: no address", options.host)))?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(10))?;
        stream.set_nodelay(true)?;
        let mut transport = Transport { stream: BufReader::new(stream), framed: false, frame: io::Cursor::new(vec![]) };
        if options.sasl {
            // a server takes an empty password as no password at all
            let password = if options.password.is_empty() { "x" } else { &options.password };
            transport.send_sasl(SASL_START, b"PLAIN")?;
            transport.send_sasl(SASL_OK, format!("\0{}\0{password}", options.user).as_bytes())?;
            let mut header = [0; 5];
            transport.stream.read_exact(&mut header).map_err(|e| {
                invalid(format!("no answer to the SASL handshake ({e}): does the server expect a bare connection?"))
            })?;
            let mut payload = vec![0; u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize];
            transport.stream.read_exact(&mut payload)?;
            if header[0] != SASL_COMPLETE {
                return Err(Error::Server(format!("authentication failed: {}", String::from_utf8_lossy(&payload))));
            }
            transport.framed = true;
        }
        let mut con = Connection { transport, session: T::Struct(vec![]), seq: 0 };
        let mut request = vec![(1, T::I32(PROTOCOL_V6))];
        if !options.user.is_empty() {
            request.push((2, string(&options.user)));
        }
        if !options.password.is_empty() {
            request.push((3, string(&options.password)));
        }
        if !options.database.is_empty() {
            // 4: configuration
            let entry = (string("use:database"), string(&options.database));
            request.push((4, T::Map(thrift::STRING, thrift::STRING, vec![entry])));
        }
        let response = con.call("OpenSession", T::Struct(request)).map_err(|e| match e {
            Error::Io(e) if !options.sasl => {
                Error::Io(invalid(format!("opening a session failed ({e}): does the server expect SASL?")))
            }
            e => e,
        })?;
        check(&response)?;
        con.session = response.field(3).cloned().ok_or_else(|| invalid("no session handle"))?;
        Ok(con)
    }

    fn call(&mut self, method: &str, request: T) -> Result<T> {
        self.seq += 1;
        self.transport.send(&thrift::call(method, self.seq, request))?;
        Ok(thrift::reply(&mut self.transport)?)
    }

    /// Run one statement to completion and fetch all of its result. A
    /// statement without one gives no columns.
    pub fn execute(&mut self, sql: &str) -> Result<ResultSet> {
        // 4: runAsync, so that a long statement is polled for, not waited on in one read
        let request = T::Struct(vec![(1, self.session.clone()), (2, string(sql)), (4, T::Bool(true))]);
        let response = self.call("ExecuteStatement", request)?;
        check(&response)?;
        let operation = response.field(2).cloned().ok_or_else(|| invalid("no operation handle"))?;
        let result = self.finish(&operation);
        // the handle is closed whatever happened; the first error is the one to report
        let closed = self.call("CloseOperation", T::Struct(vec![(1, operation)]));
        let result = result?;
        closed?;
        Ok(result)
    }

    fn finish(&mut self, operation: &T) -> Result<ResultSet> {
        let handle = || T::Struct(vec![(1, operation.clone())]);
        let mut wait = Duration::from_millis(1);
        loop {
            let status = self.call("GetOperationStatus", handle())?;
            check(&status)?;
            // TOperationState
            match status.field(2).and_then(T::as_i32) {
                // INITIALIZED, RUNNING, PENDING
                Some(0 | 1 | 7) => {
                    std::thread::sleep(wait);
                    wait = (wait * 2).min(Duration::from_millis(50));
                }
                // FINISHED
                Some(2) => break,
                state => {
                    let message =
                        status.field(5).and_then(T::as_str).map(String::from).unwrap_or_else(|| match state {
                            Some(3) => "the statement was canceled".into(),
                            Some(4) => "the statement was closed".into(),
                            Some(8) => "the statement timed out".into(),
                            _ => "the statement failed".into(),
                        });
                    return Err(Error::Server(message));
                }
            }
        }
        // TOperationHandle.hasResultSet
        if operation.field(3).and_then(T::as_bool) != Some(true) {
            return Ok(ResultSet::default());
        }
        let metadata = self.call("GetResultSetMetadata", handle())?;
        check(&metadata)?;
        let descs = metadata.field(2).and_then(|schema| schema.field(1)).map(T::as_list).unwrap_or(&[]);
        let descs: Vec<(String, String)> = descs.iter().map(column_desc).collect();
        let mut data: Vec<Values> = vec![];
        loop {
            // orientation FETCH_NEXT
            let request = T::Struct(vec![(1, operation.clone()), (2, T::I32(0)), (3, T::I64(FETCH_ROWS))]);
            let response = self.call("FetchResults", request)?;
            check(&response)?;
            let more = response.field(2).and_then(T::as_bool) == Some(true);
            // TRowSet.columns
            let columns = match response {
                T::Struct(fields) => fields.into_iter().find(|(id, _)| *id == 3).and_then(|(_, rows)| match rows {
                    T::Struct(fields) => fields.into_iter().find(|(id, _)| *id == 3).map(|(_, columns)| columns),
                    _ => None,
                }),
                _ => None,
            };
            let page = match columns {
                Some(T::List(_, columns)) => columns.into_iter().map(column_values).collect::<io::Result<Vec<_>>>()?,
                _ => vec![],
            };
            let rows = page.first().map_or(0, Values::len);
            if data.is_empty() {
                data = page;
            } else {
                for (column, more) in data.iter_mut().zip(page) {
                    column.append(more)?;
                }
            }
            // Hive and Spark never say there is more: an empty page is the end
            if rows == 0 && !more {
                break;
            }
        }
        if !data.is_empty() && data.len() != descs.len() {
            return Err(invalid(format!("{} columns described, {} sent", descs.len(), data.len())).into());
        }
        let mut data = data.into_iter();
        let columns = descs
            .into_iter()
            .map(|(name, type_name)| Column { name, type_name, values: data.next().unwrap_or(Values::String(vec![])) })
            .collect();
        Ok(ResultSet { columns })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let session = std::mem::replace(&mut self.session, T::Struct(vec![]));
        let _ = self.call("CloseSession", T::Struct(vec![(1, session)]));
    }
}
