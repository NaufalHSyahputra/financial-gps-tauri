//! Backend-neutral SQLite operations. A transaction owns one connection lease
//! and is used on its owning thread. Cursor reads borrow the current row only.
use std::future::Future;

use crate::domain::{AppError, ErrorCode};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no rows returned")]
    NoRows,
    #[error("unexpected SQL value")]
    InvalidQuery,
    #[error("unexpected SQL text value")]
    InvalidText,
    #[error("invalid stored SQL document")]
    InvalidDocument,
    #[error("SQL integer overflow")]
    IntegerOverflow,
    #[error(transparent)]
    Backend(#[from] AppError),
}

impl Error {
    fn in_row_decoder(self) -> Self {
        match self {
            Self::InvalidText => Self::InvalidQuery,
            other => other,
        }
    }
}

impl From<Error> for AppError {
    fn from(error: Error) -> Self {
        match error {
            Error::Backend(error) => error,
            Error::InvalidText => Self::new(ErrorCode::Integrity, "unexpected_database_value"),
            _ => Self::new(ErrorCode::Integrity, "database_integrity_error"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
pub enum ValueRef<'a> {
    Null,
    Integer(i64),
    Real(f64),
    Text(&'a [u8]),
    Blob(&'a [u8]),
}

impl<'a> ValueRef<'a> {
    pub fn as_str(self) -> Result<&'a str> {
        match self {
            Self::Text(value) => std::str::from_utf8(value).map_err(|_| Error::InvalidText),
            _ => Err(Error::InvalidText),
        }
    }
}

pub trait ToValue {
    fn value(&self) -> Value;
}

impl ToValue for Value {
    fn value(&self) -> Value {
        self.clone()
    }
}
impl<T: ToValue + ?Sized> ToValue for &T {
    fn value(&self) -> Value {
        (*self).value()
    }
}
impl ToValue for str {
    fn value(&self) -> Value {
        Value::Text(self.to_owned())
    }
}
impl ToValue for String {
    fn value(&self) -> Value {
        self.as_str().value()
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}
impl From<&str> for Value {
    fn from(value: &str) -> Self {
        value.value()
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}
impl<T: ToValue> ToValue for Option<T> {
    fn value(&self) -> Value {
        self.as_ref().map_or(Value::Null, ToValue::value)
    }
}
impl ToValue for bool {
    fn value(&self) -> Value {
        Value::Integer(i64::from(*self))
    }
}

pub trait FromValue: Sized {
    fn from_value(value: ValueRef<'_>) -> Result<Self>;
}
impl FromValue for String {
    fn from_value(value: ValueRef<'_>) -> Result<Self> {
        Ok(value.as_str().map_err(|_| Error::InvalidQuery)?.to_owned())
    }
}
impl FromValue for bool {
    fn from_value(value: ValueRef<'_>) -> Result<Self> {
        Ok(i64::from_value(value)? != 0)
    }
}
impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: ValueRef<'_>) -> Result<Self> {
        match value {
            ValueRef::Null => Ok(None),
            value => T::from_value(value).map(Some),
        }
    }
}
impl FromValue for Value {
    fn from_value(value: ValueRef<'_>) -> Result<Self> {
        Ok(match value {
            ValueRef::Null => Self::Null,
            ValueRef::Integer(value) => Self::Integer(value),
            ValueRef::Real(value) => Self::Real(value),
            ValueRef::Text(value) => Self::Text(
                std::str::from_utf8(value)
                    .map_err(|_| Error::InvalidQuery)?
                    .to_owned(),
            ),
            ValueRef::Blob(value) => Self::Blob(value.to_vec()),
        })
    }
}
macro_rules! integers {
    ($($type:ty),+) => {$(
        impl ToValue for $type {
            fn value(&self) -> Value { Value::Integer(i64::from(*self)) }
        }
        impl FromValue for $type {
            fn from_value(value: ValueRef<'_>) -> Result<Self> {
                match value {
                    ValueRef::Integer(value) => <Self as TryFrom<i64>>::try_from(value).map_err(|_| Error::InvalidQuery),
                    _ => Err(Error::InvalidQuery),
                }
            }
        }
    )+};
}
integers!(i64, i32, i16, i8, u32, u16, u8);

pub trait Params {
    fn into_values(self) -> Vec<Value>;
}
impl Params for [(); 0] {
    fn into_values(self) -> Vec<Value> {
        Vec::new()
    }
}
impl Params for Vec<Value> {
    fn into_values(self) -> Vec<Value> {
        self
    }
}
macro_rules! parameter_arrays {
    ($($size:literal),+) => {$(
        impl<T: ToValue> Params for [T; $size] {
            fn into_values(self) -> Vec<Value> { self.into_iter().map(|item| item.value()).collect() }
        }
    )+};
}
parameter_arrays!(1, 2, 3, 4, 5, 6, 7, 8);
pub fn params_from_iter<T: ToValue>(values: impl IntoIterator<Item = T>) -> Vec<Value> {
    values.into_iter().map(|item| item.value()).collect()
}

#[macro_export]
macro_rules! sql_params {
    ($($value:expr),* $(,)?) => {
        vec![$($crate::sql::ToValue::value(&$value)),*]
    };
}
pub use crate::sql_params as params;

pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}
impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(Error::NoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub trait RowData {
    fn value_ref(&self, column: usize) -> Result<ValueRef<'_>>;
}
pub struct Row<'a>(&'a dyn RowData);
impl<'a> Row<'a> {
    pub fn new(data: &'a dyn RowData) -> Self {
        Self(data)
    }
    pub fn get<I: TryInto<usize>, T: FromValue>(&self, index: I) -> Result<T> {
        T::from_value(
            self.0
                .value_ref(index.try_into().map_err(|_| Error::InvalidQuery)?)?,
        )
    }
    pub fn get_ref(&self, index: usize) -> Result<ValueRef<'a>> {
        self.0.value_ref(index)
    }
}

pub trait Rows {
    fn next(&mut self) -> Result<Option<Row<'_>>>;
}
pub trait SqlStatement {
    fn query(&mut self, parameters: Vec<Value>) -> Result<Box<dyn Rows + '_>>;
    fn execute(&mut self, parameters: Vec<Value>) -> Result<usize>;
}
pub trait SqlTransaction {
    fn prepare(&self, sql: &str, cached: bool) -> Result<Box<dyn SqlStatement + '_>>;
    fn execute_batch(&self, sql: &str) -> Result<()>;
}

impl<T: SqlTransaction + ?Sized> SqlTransaction for &T {
    fn prepare(&self, sql: &str, cached: bool) -> Result<Box<dyn SqlStatement + '_>> {
        (**self).prepare(sql, cached)
    }

    fn execute_batch(&self, sql: &str) -> Result<()> {
        (**self).execute_batch(sql)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionMode {
    Read,
    Immediate,
}

/// Acquire a transaction asynchronously. Inside the leased transaction, queries
/// step synchronously on the native DB thread or the owning SQLite WASM worker.
/// A lease pins a snapshot, including across nested statements on the same handle.
/// Implementations roll back an unfinished lease on drop.
pub trait SqlConnection {
    type Lease<'a>: TransactionLease + 'a
    where
        Self: 'a;
    fn begin(&mut self, mode: TransactionMode) -> impl Future<Output = Result<Self::Lease<'_>>>;
}
pub trait TransactionLease: SqlTransaction {
    fn commit(self) -> Result<()>;
    fn rollback(self) -> Result<()>;
}

pub struct Transaction<'a>(Box<dyn SqlTransaction + 'a>);
impl<'a> Transaction<'a> {
    pub fn new(backend: impl SqlTransaction + 'a) -> Self {
        Self(Box::new(backend))
    }
    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        self.0.prepare(sql, false).map(Statement)
    }
    pub fn prepare_cached(&self, sql: &str) -> Result<Statement<'_>> {
        self.0.prepare(sql, true).map(Statement)
    }
    pub fn execute(&self, sql: &str, parameters: impl Params) -> Result<usize> {
        self.prepare_cached(sql)?.execute(parameters)
    }
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql)
    }
    pub fn query_row<T>(
        &self,
        sql: &str,
        parameters: impl Params,
        read: impl FnOnce(&Row<'_>) -> Result<T>,
    ) -> Result<T> {
        self.prepare_cached(sql)?.query_row(parameters, read)
    }
}

pub struct Statement<'a>(Box<dyn SqlStatement + 'a>);
impl Statement<'_> {
    pub fn query_row<T>(
        &mut self,
        parameters: impl Params,
        read: impl FnOnce(&Row<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut rows = self.query(parameters)?;
        read(&rows.next()?.ok_or(Error::NoRows)?).map_err(Error::in_row_decoder)
    }
    pub fn query(&mut self, parameters: impl Params) -> Result<Box<dyn Rows + '_>> {
        self.0.query(parameters.into_values())
    }
    pub fn execute(&mut self, parameters: impl Params) -> Result<usize> {
        self.0.execute(parameters.into_values())
    }
    pub fn query_map<T>(
        &mut self,
        parameters: impl Params,
        mut read: impl FnMut(&Row<'_>) -> Result<T>,
    ) -> Result<impl Iterator<Item = Result<T>>> {
        let mut rows = self.query(parameters)?;
        let mut stopped = false;
        Ok(std::iter::from_fn(move || {
            if stopped {
                return None;
            }
            match rows.next() {
                Ok(Some(row)) => Some(read(&row).map_err(Error::in_row_decoder)),
                Ok(None) => None,
                Err(error) => {
                    stopped = true;
                    Some(Err(error))
                }
            }
        }))
    }
}
