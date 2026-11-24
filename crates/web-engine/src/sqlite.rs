use core_engine::{
    domain::{AppError, ErrorCode},
    sql::{Error, Result, Row, RowData, Rows, SqlStatement, SqlTransaction, Value, ValueRef},
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    pub type JsDatabase;
    #[wasm_bindgen(method, catch)]
    fn prepare(this: &JsDatabase, sql: &str) -> std::result::Result<JsStatement, JsValue>;
    #[wasm_bindgen(method, catch, js_name = executeBatch)]
    fn execute_batch(this: &JsDatabase, sql: &str) -> std::result::Result<(), JsValue>;

    type JsStatement;
    #[wasm_bindgen(method, catch)]
    fn bind(this: &JsStatement, params: &str) -> std::result::Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn next(this: &JsStatement) -> std::result::Result<Option<String>, JsValue>;
    #[wasm_bindgen(method, catch)]
    fn execute(this: &JsStatement) -> std::result::Result<u32, JsValue>;
    #[wasm_bindgen(method, catch)]
    fn close(this: &JsStatement) -> std::result::Result<(), JsValue>;
}

fn error(value: JsValue) -> Error {
    match value.as_string().as_deref() {
        Some("integer overflow") => Error::IntegerOverflow,
        Some("busy") => AppError::busy().into(),
        Some("constraint") => {
            AppError::new(ErrorCode::Integrity, "database_constraint_failed").into()
        }
        _ => AppError::new(ErrorCode::Integrity, "database_integrity_error").into(),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
enum Cell {
    Null,
    Integer(String),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
impl From<Value> for Cell {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Integer(value) => Self::Integer(value.to_string()),
            Value::Real(value) => Self::Real(value),
            Value::Text(value) => Self::Text(value),
            Value::Blob(value) => Self::Blob(value),
        }
    }
}

pub struct Session<'a>(pub &'a JsDatabase);
impl SqlTransaction for Session<'_> {
    fn prepare(&self, sql: &str, _: bool) -> Result<Box<dyn SqlStatement + '_>> {
        self.0
            .prepare(sql)
            .map(|statement| Box::new(Statement(statement)) as _)
            .map_err(error)
    }

    fn execute_batch(&self, sql: &str) -> Result<()> {
        self.0.execute_batch(sql).map_err(error)
    }
}

struct Statement(JsStatement);
impl Statement {
    fn bind(&self, values: Vec<Value>) -> Result<()> {
        let values = values.into_iter().map(Cell::from).collect::<Vec<_>>();
        let json = serde_json::to_string(&values).map_err(|_| Error::InvalidQuery)?;
        self.0.bind(&json).map_err(error)
    }
}
impl Drop for Statement {
    fn drop(&mut self) {
        let _ = self.0.close();
    }
}
impl SqlStatement for Statement {
    fn query(&mut self, parameters: Vec<Value>) -> Result<Box<dyn Rows + '_>> {
        self.bind(parameters)?;
        Ok(Box::new(Cursor {
            statement: &self.0,
            values: Vec::new(),
        }))
    }
    fn execute(&mut self, parameters: Vec<Value>) -> Result<usize> {
        self.bind(parameters)?;
        self.0.execute().map(|count| count as usize).map_err(error)
    }
}

struct Cursor<'a> {
    statement: &'a JsStatement,
    values: Vec<Cell>,
}
impl Rows for Cursor<'_> {
    fn next(&mut self) -> Result<Option<Row<'_>>> {
        let Some(json) = self.statement.next().map_err(error)? else {
            return Ok(None);
        };
        self.values = serde_json::from_str(&json).map_err(|_| Error::InvalidText)?;
        Ok(Some(Row::new(self)))
    }
}
impl RowData for Cursor<'_> {
    fn value_ref(&self, column: usize) -> Result<ValueRef<'_>> {
        Ok(match self.values.get(column).ok_or(Error::InvalidQuery)? {
            Cell::Null => ValueRef::Null,
            Cell::Integer(value) => {
                ValueRef::Integer(value.parse().map_err(|_| Error::InvalidQuery)?)
            }
            Cell::Real(value) => ValueRef::Real(*value),
            Cell::Text(value) => ValueRef::Text(value.as_bytes()),
            Cell::Blob(value) => ValueRef::Blob(value),
        })
    }
}
