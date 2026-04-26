use async_trait::async_trait;

use crate::errors::LensError;
use crate::value::LensRow;

pub mod query;

pub use query::{ColumnInfo, TableSchema};

pub mod mysql;
pub mod schema;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbKind {
    Mysql,
}

#[async_trait]
pub trait DbSource: Send + Sync {
    fn kind(&self) -> DbKind;
    fn profile_name(&self) -> &str;
    async fn list_tables(&self) -> Result<Vec<String>, LensError>;
    async fn schema(&self, table: &str) -> Result<TableSchema, LensError>;
    async fn query(&self, query: &query::CannedQuery) -> Result<Vec<LensRow>, LensError>;
}
