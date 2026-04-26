use schemars::JsonSchema;
use serde::Deserialize;
use thiserror::Error;

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
    #[serde(default = "default_column_allowed")]
    pub allowed: bool,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize, JsonSchema)]
pub struct CannedQuery {
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
    #[serde(default)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledQuery {
    pub sql: String,
    pub binds: Vec<QueryValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Mysql,
    Postgres,
    Sqlite,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QueryValue {
    String(String),
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(bool),
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum QueryError {
    #[error("query table `{query_table}` does not match schema table `{schema_table}`")]
    TableMismatch {
        query_table: String,
        schema_table: String,
    },
    #[error("unknown column `{0}`")]
    UnknownColumn(String),
    #[error("column `{0}` is not allowed by schema policy")]
    DisallowedColumn(String),
    #[error("operator `{op:?}` cannot be used with column `{col}` of type `{data_type}`")]
    OperatorTypeMismatch {
        col: String,
        op: WhereOp,
        data_type: String,
    },
    #[error("operator `{0:?}` requires a list value")]
    OperatorRequiresList(WhereOp),
    #[error("operator `{0:?}` requires a scalar value")]
    OperatorRequiresScalar(WhereOp),
    #[error("operator `{0:?}` requires no value")]
    OperatorRequiresNoValue(WhereOp),
    #[error("operator `{0:?}` requires a value")]
    OperatorRequiresValue(WhereOp),
    #[error("unsafe column identifier `{0}`")]
    UnsafeColumn(String),
}

impl CannedQuery {
    pub fn compile_to_sql(&self, table_schema: &TableSchema) -> Result<CompiledQuery, QueryError> {
        self.compile_to_sql_for(table_schema, Dialect::Mysql)
    }

    pub fn compile_to_sql_for(
        &self,
        table_schema: &TableSchema,
        dialect: Dialect,
    ) -> Result<CompiledQuery, QueryError> {
        if self.table != table_schema.table {
            return Err(QueryError::TableMismatch {
                query_table: self.table.clone(),
                schema_table: table_schema.table.clone(),
            });
        }

        let selected_columns = match &self.columns {
            Some(columns) if !columns.is_empty() => columns.clone(),
            _ => table_schema
                .columns
                .iter()
                .filter(|column| column.allowed)
                .map(|column| column.name.clone())
                .collect(),
        };

        for column in &selected_columns {
            validate_column(column, table_schema)?;
        }

        let select_list = if selected_columns.is_empty() {
            "1".to_string()
        } else {
            selected_columns
                .iter()
                .map(|column| escape_ident(column, dialect))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let mut sql = format!(
            "SELECT {select_list} FROM {}",
            escape_ident(&table_schema.table, dialect)
        );
        let mut binds = Vec::new();
        let mut placeholders = PlaceholderState::new(dialect);

        if let Some(where_clauses) = &self.r#where
            && !where_clauses.is_empty()
        {
            let combinator = match self.where_combinator.unwrap_or(WhereCombinator::And) {
                WhereCombinator::And => " AND ",
                WhereCombinator::Or => " OR ",
            };
            let mut parts = Vec::with_capacity(where_clauses.len());
            for clause in where_clauses {
                let column = validate_column(&clause.col, table_schema)?;
                parts.push(compile_where_clause(
                    clause,
                    column,
                    &mut binds,
                    &mut placeholders,
                    dialect,
                )?);
            }
            sql.push_str(" WHERE ");
            sql.push_str(&parts.join(combinator));
        }

        if let Some(order_by) = &self.order_by
            && !order_by.is_empty()
        {
            let mut parts = Vec::with_capacity(order_by.len());
            for order in order_by {
                validate_column(&order.col, table_schema)?;
                let dir = match order.dir {
                    OrderDir::Asc => "ASC",
                    OrderDir::Desc => "DESC",
                };
                parts.push(format!("{} {dir}", escape_ident(&order.col, dialect)));
            }
            sql.push_str(" ORDER BY ");
            sql.push_str(&parts.join(", "));
        }

        let hard_cap = table_schema.limit_cap.unwrap_or(u32::MAX);
        let limit = self.limit.unwrap_or(hard_cap).min(hard_cap);
        sql.push_str(" LIMIT ");
        sql.push_str(&placeholders.next());
        binds.push(QueryValue::U64(limit as u64));

        Ok(CompiledQuery { sql, binds })
    }
}

fn compile_where_clause(
    clause: &WhereClause,
    column: &ColumnInfo,
    binds: &mut Vec<QueryValue>,
    placeholders: &mut PlaceholderState,
    dialect: Dialect,
) -> Result<String, QueryError> {
    if !operator_matches_type(clause.op, &column.data_type) {
        return Err(QueryError::OperatorTypeMismatch {
            col: clause.col.clone(),
            op: clause.op,
            data_type: column.data_type.clone(),
        });
    }

    let ident = escape_ident(&clause.col, dialect);
    match clause.op {
        WhereOp::IsNull | WhereOp::IsNotNull => {
            if clause.val.is_some() {
                return Err(QueryError::OperatorRequiresNoValue(clause.op));
            }
            Ok(format!(
                "{ident} IS {}NULL",
                if clause.op == WhereOp::IsNotNull {
                    "NOT "
                } else {
                    ""
                }
            ))
        }
        WhereOp::In => {
            let values = match clause.val.as_ref() {
                Some(ScalarOrList::List(values)) => values,
                Some(ScalarOrList::Scalar(_)) => {
                    return Err(QueryError::OperatorRequiresList(clause.op));
                }
                None => return Err(QueryError::OperatorRequiresValue(clause.op)),
            };
            if values.is_empty() {
                return Err(QueryError::OperatorRequiresList(clause.op));
            }
            for value in values {
                binds.push(query_value(value, clause.op)?);
            }
            let placeholders = (0..values.len())
                .map(|_| placeholders.next())
                .collect::<Vec<_>>()
                .join(", ");
            Ok(format!("{ident} IN ({placeholders})",))
        }
        WhereOp::Eq
        | WhereOp::Ne
        | WhereOp::Gt
        | WhereOp::Gte
        | WhereOp::Lt
        | WhereOp::Lte
        | WhereOp::Like => {
            let value = match clause.val.as_ref() {
                Some(ScalarOrList::Scalar(value)) => value,
                Some(ScalarOrList::List(_)) => {
                    return Err(QueryError::OperatorRequiresScalar(clause.op));
                }
                None => return Err(QueryError::OperatorRequiresValue(clause.op)),
            };
            binds.push(query_value(value, clause.op)?);
            Ok(format!(
                "{ident} {} {}",
                sql_operator(clause.op),
                placeholders.next()
            ))
        }
    }
}

fn query_value(value: &ScalarValue, op: WhereOp) -> Result<QueryValue, QueryError> {
    match value {
        ScalarValue::String(value) => Ok(QueryValue::String(value.clone())),
        ScalarValue::I64(value) => Ok(QueryValue::I64(*value)),
        ScalarValue::U64(value) => Ok(QueryValue::U64(*value)),
        ScalarValue::F64(value) => Ok(QueryValue::F64(*value)),
        ScalarValue::Bool(value) => Ok(QueryValue::Bool(*value)),
        ScalarValue::Null => Err(QueryError::OperatorRequiresNoValue(op)),
    }
}

fn sql_operator(op: WhereOp) -> &'static str {
    match op {
        WhereOp::Eq => "=",
        WhereOp::Ne => "!=",
        WhereOp::Gt => ">",
        WhereOp::Gte => ">=",
        WhereOp::Lt => "<",
        WhereOp::Lte => "<=",
        WhereOp::Like => "LIKE",
        WhereOp::In | WhereOp::IsNull | WhereOp::IsNotNull => unreachable!("handled separately"),
    }
}

fn validate_column<'a>(
    column: &str,
    table_schema: &'a TableSchema,
) -> Result<&'a ColumnInfo, QueryError> {
    if has_sql_keyword(column) || column.contains(';') || column.contains("--") {
        return Err(QueryError::UnsafeColumn(column.to_string()));
    }
    let info = table_schema
        .columns
        .iter()
        .find(|candidate| candidate.name == column)
        .ok_or_else(|| QueryError::UnknownColumn(column.to_string()))?;
    if !info.allowed {
        return Err(QueryError::DisallowedColumn(column.to_string()));
    }
    Ok(info)
}

fn has_sql_keyword(column: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "alter", "delete", "drop", "insert", "select", "truncate", "union", "update",
    ];
    column
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| KEYWORDS.contains(&part.to_ascii_lowercase().as_str()))
}

fn operator_matches_type(op: WhereOp, data_type: &str) -> bool {
    if op == WhereOp::Like {
        is_string_family(data_type)
    } else {
        true
    }
}

fn is_string_family(data_type: &str) -> bool {
    let ty = data_type.to_ascii_lowercase();
    matches!(
        ty.as_str(),
        "char" | "varchar" | "text" | "tinytext" | "mediumtext" | "longtext" | "json"
    )
}

fn escape_ident(ident: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::Mysql | Dialect::Sqlite => format!("`{}`", ident.replace('`', "``")),
        Dialect::Postgres => format!("\"{}\"", ident.replace('"', "\"\"")),
    }
}

struct PlaceholderState {
    dialect: Dialect,
    next_index: usize,
}

impl PlaceholderState {
    fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            next_index: 1,
        }
    }

    fn next(&mut self) -> String {
        match self.dialect {
            Dialect::Mysql | Dialect::Sqlite => "?".to_string(),
            Dialect::Postgres => {
                let placeholder = format!("${}", self.next_index);
                self.next_index += 1;
                placeholder
            }
        }
    }
}

fn default_column_allowed() -> bool {
    true
}
