//! Legacy public DTOs, retained only for the green development assembly.
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableSchema {
    #[serde(skip)]
    pub table: String,
    pub table_token: String,
    pub columns: Vec<ColumnInfo>,
    #[serde(default)]
    pub limit_cap: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ColumnInfo {
    #[serde(skip)]
    pub name: String,
    pub name_token: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub allowed: bool,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, JsonSchema)]
pub struct CannedQuery {
    #[schemars(
        description = "Configured profile name selecting the source to dispatch. Required. Pattern: ^[a-z0-9][a-z0-9_-]{0,63}$.",
        regex(pattern = r"^[a-z0-9][a-z0-9_-]{0,63}$")
    )]
    pub profile: String,
    pub table: String,
    pub columns: Option<Vec<String>>,
    #[serde(default)]
    pub r#where: Option<Vec<WhereClause>>,
    #[serde(default)]
    pub where_combinator: Option<WhereCombinator>,
    #[serde(default)]
    pub order_by: Option<Vec<OrderBy>>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, JsonSchema)]
pub struct WhereClause {
    pub col: String,
    pub op: WhereOp,
    #[serde(default, alias = "value")]
    pub val: Option<ScalarOrList>,
}

#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WhereOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Like,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WhereCombinator {
    And,
    Or,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, JsonSchema)]
pub struct OrderBy {
    pub col: String,
    pub dir: OrderDir,
}

#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrderDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum ScalarOrList {
    Scalar(ScalarValue),
    List(Vec<ScalarValue>),
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum ScalarValue {
    String(String),
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(bool),
    Null,
}
