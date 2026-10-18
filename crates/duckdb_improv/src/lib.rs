//! `improv` as a DuckDB loadable extension: Improv's formula language and
//! multidimensional models over DuckDB data.
//!
//!   - `improv_calc(formula [, inputs])`      scalar -> DOUBLE
//!   - `improv_calc_text(formula [, inputs])` scalar -> VARCHAR
//!   - `improv_functions([query := ...])`     table: the built-in functions
//!   - `improv(source, model)`                table: one row per derived cell
//!
//! The logic lives in `improv_duckdb_model`; this crate moves values across
//! the C API. `improv(...)` reads its source with SQL at bind time, on a pool
//! of connections cloned from the one DuckDB hands the entrypoint
//! (`IMPROV_DUCKDB_POOL`, default one per core up to 8), the way mentat's
//! DuckDB extension does.
//!
//! EXTENSION-ONLY: the `loadable-extension` feature replaces the DuckDB C API
//! with the function pointers DuckDB passes in; a normal client
//! `Connection::open` here panics with "API not initialized".

use std::error::Error;
use std::sync::{Condvar, Mutex, OnceLock};

use duckdb::core::{DataChunkHandle, Inserter, LogicalTypeHandle, LogicalTypeId};
use duckdb::ffi::duckdb_string_t;
use duckdb::types::{DuckString, ValueRef};
use duckdb::vscalar::{ScalarFunctionSignature, VScalar};
use duckdb::vtab::{arrow::WritableVector, BindInfo, InitInfo, TableFunctionInfo, VTab};
use duckdb::{duckdb_entrypoint_c_api, Connection, Result};
use improv_duckdb_model::calc::{self, Cache};
use improv_duckdb_model::{self as dm, CellValue, ValueType};

type BoxErr = Box<dyn Error>;

// ---------------------------------------------------------------------------
// Connections (mentat's Pool / Lease).
// ---------------------------------------------------------------------------

/// One of the extension's connections to the host database.
#[derive(Clone, Copy)]
struct Conn(&'static Connection);

// SAFETY: a `duckdb::Connection` is `Send` but not `Sync` (its statement
// cache is a RefCell). A `Conn` is in the pool or leased to exactly one call
// (`conn()`), so no two threads use one connection at once.
unsafe impl Send for Conn {}

/// The extension's connections, made in the entrypoint: the database handle
/// DuckDB passes it is only valid during that call. An `improv(...)` bind
/// leases one for as long as it reads its source.
struct Pool {
    free: Mutex<Vec<Conn>>,
    returned: Condvar,
}

static POOL: OnceLock<Pool> = OnceLock::new();

/// Connections to open: `IMPROV_DUCKDB_POOL`, else one per core up to 8.
fn pool_size() -> usize {
    std::env::var("IMPROV_DUCKDB_POOL")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(8)))
}

/// A leased connection; dropping it returns the connection to the pool.
struct Lease(Conn);

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(pool) = POOL.get() {
            pool.free
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(self.0);
            pool.returned.notify_one();
        }
    }
}

/// A free connection, waiting for one if all are leased.
fn conn() -> Result<Lease, BoxErr> {
    let pool = POOL.get().ok_or("improv: extension not initialized")?;
    let mut free = pool.free.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if let Some(c) = free.pop() {
            return Ok(Lease(c));
        }
        free = pool.returned.wait(free).unwrap_or_else(|e| e.into_inner());
    }
}

// ---------------------------------------------------------------------------
// Shared plumbing.
// ---------------------------------------------------------------------------

fn varchar() -> LogicalTypeHandle {
    LogicalTypeId::Varchar.into()
}

fn double() -> LogicalTypeHandle {
    LogicalTypeId::Double.into()
}

/// Row `i` of VARCHAR column `col`; `None` if NULL.
fn str_arg(input: &DataChunkHandle, col: usize, i: usize) -> Option<String> {
    let v = input.flat_vector(col);
    if v.row_is_null(i as u64) {
        return None;
    }
    // SAFETY: column `col` is VARCHAR (every signature below) with
    // `input.len()` rows, and `i < input.len()`.
    let mut s = unsafe { v.as_slice_with_len::<duckdb_string_t>(input.len()) }[i];
    Some(DuckString::new(&mut s).as_str().to_string())
}

/// A VARCHAR bind parameter; `None` if SQL NULL.
fn param(bind: &BindInfo, i: u64) -> Option<String> {
    let v = bind.get_parameter(i);
    (!v.is_null()).then(|| v.to_string())
}

/// DuckDB's STANDARD_VECTOR_SIZE: the output chunk's capacity.
const VECTOR_SIZE: usize = 2048;

/// A table function's cursor over its materialized rows.
struct Cursor(Mutex<usize>);

impl Cursor {
    /// The next batch `start..end` (empty when done), advancing past it.
    fn next(&self, total: usize) -> std::ops::Range<usize> {
        let mut c = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let start = *c;
        *c = total.min(start + VECTOR_SIZE);
        start..*c
    }
}

/// One scan thread: the rows are materialized at bind time and a single
/// cursor streams them.
fn init_cursor(info: &InitInfo) -> Result<Cursor, BoxErr> {
    info.set_max_threads(1);
    Ok(Cursor(Mutex::new(0)))
}

/// Write `v` (or NULL) at `row` of output column `col`.
fn put_str(output: &DataChunkHandle, col: usize, row: usize, v: Option<&str>) {
    let mut vec = output.flat_vector(col);
    match v {
        Some(s) => vec.insert(row, s),
        None => vec.set_null(row),
    }
}

// ---------------------------------------------------------------------------
// improv_calc / improv_calc_text.
// ---------------------------------------------------------------------------

/// Compiled formulas, shared by every thread and both scalar functions.
static CACHE: OnceLock<Cache> = OnceLock::new();

fn cache() -> &'static Cache {
    CACHE.get_or_init(Cache::default)
}

/// Run `f` on each row's value: NULL formula -> NULL; a formula that does
/// not compile (or bad inputs JSON) fails the query.
fn calc_rows(
    input: &DataChunkHandle,
    mut put: impl FnMut(usize, Option<CellValue>),
) -> Result<(), BoxErr> {
    let has_inputs = input.num_columns() > 1;
    for i in 0..input.len() {
        let Some(formula) = str_arg(input, 0, i) else {
            put(i, None);
            continue;
        };
        let inputs = if has_inputs {
            str_arg(input, 1, i)
        } else {
            None
        };
        put(i, cache().calc(&formula, inputs.as_deref())?);
    }
    Ok(())
}

fn calc_signatures(ret: LogicalTypeId) -> Vec<ScalarFunctionSignature> {
    vec![
        ScalarFunctionSignature::exact(vec![varchar()], ret.into()),
        ScalarFunctionSignature::exact(vec![varchar(), varchar()], ret.into()),
    ]
}

struct ImprovCalc;

impl VScalar for ImprovCalc {
    type State = ();

    fn invoke(
        _: &(),
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), BoxErr> {
        let mut out = output.flat_vector();
        let mut values = vec![0f64; input.len()];
        calc_rows(input, |i, v| match v.as_ref().and_then(calc::number) {
            Some(n) => values[i] = n,
            None => out.set_null(i),
        })?;
        // SAFETY: the output is a DOUBLE vector with room for `input.len()`.
        unsafe { out.as_mut_slice_with_len::<f64>(values.len()) }
            .iter_mut()
            .zip(&values)
            .for_each(|(o, v)| *o = *v);
        Ok(())
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        calc_signatures(LogicalTypeId::Double)
    }
}

struct ImprovCalcText;

impl VScalar for ImprovCalcText {
    type State = ();

    fn invoke(
        _: &(),
        input: &mut DataChunkHandle,
        output: &mut dyn WritableVector,
    ) -> Result<(), BoxErr> {
        let mut out = output.flat_vector();
        calc_rows(input, |i, v| match v {
            Some(v) => out.insert(i, v.to_string().as_str()),
            None => out.set_null(i),
        })
    }

    fn signatures() -> Vec<ScalarFunctionSignature> {
        calc_signatures(LogicalTypeId::Varchar)
    }
}

// ---------------------------------------------------------------------------
// improv_functions([query := ...]).
// ---------------------------------------------------------------------------

struct FunctionsBind(Vec<dm::FunctionRow>);

const FUNCTION_COLUMNS: [&str; 6] = [
    "name",
    "category",
    "signature",
    "returns",
    "description",
    "aliases",
];

fn functions_bind(bind: &BindInfo, query: Option<&str>) -> FunctionsBind {
    for c in FUNCTION_COLUMNS {
        bind.add_result_column(c, varchar());
    }
    FunctionsBind(dm::function_rows(query))
}

fn functions_func(
    rows: &[dm::FunctionRow],
    cursor: &Cursor,
    output: &mut DataChunkHandle,
) -> Result<(), BoxErr> {
    let range = cursor.next(rows.len());
    for (r, f) in rows[range.clone()].iter().enumerate() {
        put_str(output, 0, r, Some(f.name));
        put_str(output, 1, r, Some(f.category));
        put_str(output, 2, r, Some(&f.signature));
        put_str(output, 3, r, Some(f.returns));
        put_str(output, 4, r, Some(f.description));
        put_str(output, 5, r, f.aliases.as_deref());
    }
    output.set_len(range.len());
    Ok(())
}

struct ImprovFunctions;

impl VTab for ImprovFunctions {
    type InitData = Cursor;
    type BindData = FunctionsBind;

    /// `improv_functions(query := 'pmt')` filters by `functions::search`
    /// (best match first). The C API registers one table function per
    /// name, so the filter is a named parameter, not an overload.
    fn bind(bind: &BindInfo) -> Result<FunctionsBind, BoxErr> {
        let query = bind
            .get_named_parameter("query")
            .filter(|v| !v.is_null())
            .map(|v| v.to_string());
        Ok(functions_bind(bind, query.as_deref()))
    }

    fn init(info: &InitInfo) -> Result<Cursor, BoxErr> {
        init_cursor(info)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<(), BoxErr> {
        functions_func(&func.get_bind_data().0, func.get_init_data(), output)
    }

    fn named_parameters() -> Option<Vec<(String, LogicalTypeHandle)>> {
        Some(vec![("query".into(), varchar())])
    }
}

// ---------------------------------------------------------------------------
// improv(source, model).
// ---------------------------------------------------------------------------

/// Run `sql` on `c` and map each row.
fn query<T>(
    c: &Connection,
    sql: &str,
    mut f: impl FnMut(&duckdb::Row<'_>) -> Result<T, BoxErr>,
) -> Result<Vec<T>, BoxErr> {
    let mut stmt = c.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(f(row)?);
    }
    Ok(out)
}

/// Column `i` of `row` as text; `None` if NULL.
fn text_at(row: &duckdb::Row<'_>, i: usize) -> Result<Option<String>, BoxErr> {
    Ok(row.get::<_, Option<String>>(i)?)
}

/// Column `i` of `row` as an input value of type `vt` (the input SQL cast
/// it: DOUBLE, BOOLEAN or VARCHAR).
fn value_at(row: &duckdb::Row<'_>, i: usize, vt: ValueType) -> Result<Option<CellValue>, BoxErr> {
    Ok(match (vt, row.get_ref(i)?) {
        (_, ValueRef::Null) => None,
        (ValueType::Boolean, _) => Some(CellValue::Bool(row.get::<_, bool>(i)?)),
        (ValueType::Text, _) => text_at(row, i)?.map(CellValue::Text),
        (_, _) => Some(CellValue::num(row.get::<_, f64>(i)?)),
    })
}

struct ModelBind {
    ncats: usize,
    rows: Vec<dm::Row>,
}

/// Read the source and evaluate the model (all at bind time: the columns
/// depend on the spec, and the result is small next to the source).
fn evaluate(source: &str, model: &str) -> Result<(dm::Spec, Vec<dm::Row>), BoxErr> {
    let spec = dm::parse_spec(model)?;
    let lease = conn()?;
    let c = lease.0 .0;
    let columns = query(c, &dm::describe_sql(source), |r| {
        Ok(dm::Column {
            name: r.get(0)?,
            ty: r.get(1)?,
        })
    })
    .map_err(|e| format!("improv: cannot read source {source:?}: {e}"))?;
    let bound = dm::bind(&spec, &columns)?;
    let items = bound
        .dims
        .iter()
        .map(|d| {
            query(c, &dm::items_sql(source, d), |r| {
                Ok(text_at(r, 0)?.unwrap_or_default())
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ndims = bound.dims.len();
    let mut facts = Vec::with_capacity(bound.inputs.len());
    for (i, (_, vt)) in bound.inputs.iter().enumerate() {
        let sql = dm::input_sql(source, &spec, &bound, i)?;
        facts.push(query(c, &sql, |r| {
            let dims = (0..ndims)
                .map(|d| text_at(r, d))
                .collect::<Result<_, _>>()?;
            Ok((dims, value_at(r, ndims, *vt)?))
        })?);
    }
    drop(lease);
    let rows = dm::run(&spec, &bound, &items, facts)?;
    Ok((spec, rows))
}

struct ImprovModel;

impl VTab for ImprovModel {
    type InitData = Cursor;
    type BindData = ModelBind;

    fn bind(bind: &BindInfo) -> Result<ModelBind, BoxErr> {
        let source = param(bind, 0).ok_or("improv: source must not be NULL")?;
        let model = param(bind, 1).ok_or("improv: model must not be NULL")?;
        let (spec, rows) = evaluate(&source, &model)?;
        bind.add_result_column("measure", varchar());
        for c in &spec.categories {
            bind.add_result_column(&c.name, varchar());
        }
        bind.add_result_column("value", double());
        bind.add_result_column("value_text", varchar());
        Ok(ModelBind {
            ncats: spec.categories.len(),
            rows,
        })
    }

    fn init(info: &InitInfo) -> Result<Cursor, BoxErr> {
        init_cursor(info)
    }

    fn func(func: &TableFunctionInfo<Self>, output: &mut DataChunkHandle) -> Result<(), BoxErr> {
        let b = func.get_bind_data();
        let range = func.get_init_data().next(b.rows.len());
        let mut values = output.flat_vector(b.ncats + 1);
        for (r, row) in b.rows[range.clone()].iter().enumerate() {
            put_str(output, 0, r, Some(&row.measure));
            for (c, d) in row.dims.iter().enumerate() {
                put_str(output, c + 1, r, d.as_deref());
            }
            match row.value {
                // SAFETY: column ncats + 1 is DOUBLE, `r` < VECTOR_SIZE.
                Some(v) => unsafe { values.as_mut_slice::<f64>()[r] = v },
                None => values.set_null(r),
            }
            put_str(output, b.ncats + 2, r, Some(&row.text));
        }
        output.set_len(range.len());
        Ok(())
    }

    fn parameters() -> Option<Vec<LogicalTypeHandle>> {
        Some(vec![varchar(), varchar()]) // source, model
    }
}

#[duckdb_entrypoint_c_api]
pub unsafe fn extension_entrypoint(con: Connection) -> Result<(), Box<dyn Error>> {
    if POOL.get().is_none() {
        let mut free = Vec::new();
        for _ in 0..pool_size() {
            free.push(Conn(Box::leak(Box::new(con.try_clone()?))));
        }
        let _ = POOL.set(Pool {
            free: Mutex::new(free),
            returned: Condvar::new(),
        });
    }
    con.register_scalar_function::<ImprovCalc>("improv_calc")?;
    con.register_scalar_function::<ImprovCalcText>("improv_calc_text")?;
    con.register_table_function::<ImprovFunctions>("improv_functions")?;
    con.register_table_function::<ImprovModel>("improv")?;
    Ok(())
}
