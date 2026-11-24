use core_engine::sql::{
    Error, Result, Row, RowData, Rows, SqlConnection, SqlStatement, SqlTransaction,
    TransactionLease, TransactionMode, Value, ValueRef,
};
use rusqlite::{
    CachedStatement, Connection, Statement, Transaction, TransactionBehavior,
    fallible_streaming_iterator::FallibleStreamingIterator,
    params_from_iter,
    types::{ToSql, ToSqlOutput, ValueRef as SqliteValue},
};

use super::database::sql_error;

fn error(source: rusqlite::Error) -> Error {
    match source {
        rusqlite::Error::QueryReturnedNoRows => Error::NoRows,
        rusqlite::Error::SqliteFailure(_, Some(ref message)) if message == "integer overflow" => {
            Error::IntegerOverflow
        }
        error => Error::Backend(sql_error(error)),
    }
}

pub fn view(connection: &Connection) -> core_engine::sql::Transaction<'_> {
    core_engine::sql::Transaction::new(SqliteSession(connection))
}

pub struct SqliteSession<'a>(pub &'a Connection);
impl SqlTransaction for SqliteSession<'_> {
    fn prepare(&self, sql: &str, cached: bool) -> Result<Box<dyn SqlStatement + '_>> {
        prepare(self.0, sql, cached)
    }

    fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql).map_err(error)
    }
}

pub struct SqliteConnection<'a>(pub &'a mut Connection);
pub struct SqliteLease<'a>(Transaction<'a>);

impl SqliteConnection<'_> {
    pub fn begin_on_worker(&mut self, mode: TransactionMode) -> Result<SqliteLease<'_>> {
        let behavior = match mode {
            TransactionMode::Read => TransactionBehavior::Deferred,
            TransactionMode::Immediate => TransactionBehavior::Immediate,
        };
        Ok(SqliteLease(
            self.0.transaction_with_behavior(behavior).map_err(error)?,
        ))
    }
}

impl SqlConnection for SqliteConnection<'_> {
    type Lease<'a>
        = SqliteLease<'a>
    where
        Self: 'a;

    async fn begin(&mut self, mode: TransactionMode) -> Result<Self::Lease<'_>> {
        self.begin_on_worker(mode)
    }
}

impl<'a> SqliteLease<'a> {
    pub(super) fn into_native(self) -> Transaction<'a> {
        self.0
    }
}

impl SqlTransaction for SqliteLease<'_> {
    fn prepare(&self, sql: &str, cached: bool) -> Result<Box<dyn SqlStatement + '_>> {
        prepare(&self.0, sql, cached)
    }

    fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql).map_err(error)
    }
}

impl TransactionLease for SqliteLease<'_> {
    fn commit(self) -> Result<()> {
        self.0.commit().map_err(error)
    }

    fn rollback(self) -> Result<()> {
        self.0.rollback().map_err(error)
    }
}

fn prepare<'a>(
    connection: &'a Connection,
    sql: &str,
    cached: bool,
) -> Result<Box<dyn SqlStatement + 'a>> {
    let statement = if cached {
        NativeStatement::Cached(connection.prepare_cached(sql).map_err(error)?)
    } else {
        NativeStatement::Plain(connection.prepare(sql).map_err(error)?)
    };
    Ok(Box::new(statement))
}

enum NativeStatement<'a> {
    Cached(CachedStatement<'a>),
    Plain(Statement<'a>),
}
impl<'a> NativeStatement<'a> {
    fn statement(&mut self) -> &mut Statement<'a> {
        match self {
            Self::Cached(statement) => statement,
            Self::Plain(statement) => statement,
        }
    }
}
impl SqlStatement for NativeStatement<'_> {
    fn query(&mut self, values: Vec<Value>) -> Result<Box<dyn Rows + '_>> {
        let rows = self
            .statement()
            .query(params_from_iter(values.iter().map(Parameter)))
            .map_err(error)?;
        Ok(Box::new(NativeRows(rows)))
    }

    fn execute(&mut self, values: Vec<Value>) -> Result<usize> {
        self.statement()
            .execute(params_from_iter(values.iter().map(Parameter)))
            .map_err(error)
    }
}

struct Parameter<'a>(&'a Value);
impl ToSql for Parameter<'_> {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(match self.0 {
            Value::Null => SqliteValue::Null,
            Value::Integer(value) => SqliteValue::Integer(*value),
            Value::Real(value) => SqliteValue::Real(*value),
            Value::Text(value) => SqliteValue::Text(value.as_bytes()),
            Value::Blob(value) => SqliteValue::Blob(value),
        }))
    }
}

struct NativeRows<'a>(rusqlite::Rows<'a>);
impl Rows for NativeRows<'_> {
    fn next(&mut self) -> Result<Option<Row<'_>>> {
        let available = self.0.next().map_err(error)?.is_some();
        Ok(available.then(|| Row::new(self)))
    }
}
impl RowData for NativeRows<'_> {
    fn value_ref(&self, column: usize) -> Result<ValueRef<'_>> {
        let row = self.0.get().ok_or(Error::NoRows)?;
        Ok(match row.get_ref(column).map_err(error)? {
            SqliteValue::Null => ValueRef::Null,
            SqliteValue::Integer(value) => ValueRef::Integer(value),
            SqliteValue::Real(value) => ValueRef::Real(value),
            SqliteValue::Text(value) => ValueRef::Text(value),
            SqliteValue::Blob(value) => ValueRef::Blob(value),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_engine::sql::{Transaction as Sql, params};

    #[test]
    fn async_leases_commit_and_rollback_on_sqlite() {
        let directory = tempfile::tempdir().unwrap();
        let mut database = Connection::open(directory.path().join("lease.db")).unwrap();
        database
            .execute_batch("CREATE TABLE amounts(id TEXT PRIMARY KEY, minor INTEGER) STRICT")
            .unwrap();
        let mut backend = SqliteConnection(&mut database);

        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                let lease = backend.begin(TransactionMode::Immediate).await.unwrap();
                Sql::new(&lease)
                    .execute(
                        "INSERT INTO amounts VALUES(?1,?2)",
                        params!["saved", 9_007_199_254_740_993_i64],
                    )
                    .unwrap();
                lease.commit().unwrap();

                let lease = backend.begin(TransactionMode::Immediate).await.unwrap();
                Sql::new(&lease)
                    .execute("INSERT INTO amounts VALUES(?1,NULL)", ["rolled back"])
                    .unwrap();
                lease.rollback().unwrap();

                let lease = backend.begin(TransactionMode::Read).await.unwrap();
                let sql = Sql::new(&lease);
                let mut statement = sql.prepare("SELECT id, minor FROM amounts").unwrap();
                let rows: Vec<(String, Option<i64>)> = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<Result<_>>()
                    .unwrap();
                assert_eq!(
                    rows,
                    vec![("saved".to_owned(), Some(9_007_199_254_740_993))]
                );
            });
    }
}
