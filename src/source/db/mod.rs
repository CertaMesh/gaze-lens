use async_trait::async_trait;

use crate::errors::LensError;
use crate::value::LensRow;

pub mod query;

pub use query::{ColumnInfo, TableSchema};

pub mod mysql;
pub mod postgres;
pub mod runtime;
pub mod schema;
pub mod sqlite;

pub(crate) use runtime::connect_db_source_with_password;
pub use runtime::{DbRuntimePlan, connect_db_source, runtime_plan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbKind {
    Mysql,
    Postgres,
    Sqlite,
}

#[async_trait]
pub trait DbSource: Send + Sync {
    fn kind(&self) -> DbKind;
    fn profile_name(&self) -> &str;
    async fn list_tables(&self) -> Result<Vec<String>, LensError>;
    async fn schema(&self, table: &str) -> Result<TableSchema, LensError>;
    async fn query(&self, query: &query::CannedQuery) -> Result<Vec<LensRow>, LensError>;
}
