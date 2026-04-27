use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};
use sqlx::{Column, ConnectOptions, Row, TypeInfo, ValueRef};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::errors::LensError;
use crate::profile::{Profile, SourceSpec};
use crate::source::db::query::{CannedQuery, Dialect, QueryValue};
use crate::value::{LensRow, LensValue, LowerError};

use super::{ColumnInfo, DbKind, DbSource, TableSchema};

pub struct SqliteSource {
    pool: SqlitePool,
    profile_name: String,
    limit_cap: u32,
    json_text_columns: BTreeSet<String>,
}

impl std::fmt::Debug for SqliteSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSource")
            .field("profile_name", &self.profile_name)
            .field("limit_cap", &self.limit_cap)
            .finish_non_exhaustive()
    }
}

impl SqliteSource {
    pub async fn connect(profile: &Profile, limit_cap: u32) -> Result<Self, LensError> {
        let SourceSpec::Sqlite {
            path,
            readonly_required,
            json_text_columns,
        } = &profile.source
        else {
            return Err(LensError::Profile {
                detail: format!("profile `{}` is not sqlite", profile.name),
            });
        };
        if !readonly_required {
            return Err(LensError::Profile {
                detail: format!(
                    "sqlite profile `{}` must require read-only mode",
                    profile.name
                ),
            });
        }
        let uri = format!("sqlite:{}", path.to_string_lossy());
        let options = SqliteConnectOptions::from_str(&uri)
            .map_err(|err| source_error(&profile.name, err.to_string(), None))?
            .read_only(true)
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|err| source_error(&profile.name, err.to_string(), None))?;
        Ok(Self {
            pool,
            profile_name: profile.name.clone(),
            limit_cap,
            json_text_columns: json_text_columns.iter().cloned().collect(),
        })
    }

    #[doc(hidden)]
    pub fn from_pool_for_tests(
        pool: SqlitePool,
        profile_name: impl Into<String>,
        limit_cap: u32,
    ) -> Self {
        Self {
            pool,
            profile_name: profile_name.into(),
            limit_cap,
            json_text_columns: BTreeSet::new(),
        }
    }

    #[doc(hidden)]
    pub fn from_pool_for_tests_with_json_text_columns(
        pool: SqlitePool,
        profile_name: impl Into<String>,
        limit_cap: u32,
        json_text_columns: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            pool,
            profile_name: profile_name.into(),
            limit_cap,
            json_text_columns: json_text_columns
                .into_iter()
                .map(Into::into)
                .collect::<BTreeSet<_>>(),
        }
    }
}

#[async_trait]
impl DbSource for SqliteSource {
    fn kind(&self) -> DbKind {
        DbKind::Sqlite
    }

    fn profile_name(&self) -> &str {
        &self.profile_name
    }

    async fn list_tables(&self) -> Result<Vec<String>, LensError> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|err| source_error(&self.profile_name, err.to_string(), None))?;
        Ok(rows.into_iter().map(|(table,)| table).collect())
    }

    async fn schema(&self, table: &str) -> Result<TableSchema, LensError> {
        let sql = format!("PRAGMA table_info({})", escape_ident(table));
        let rows = sqlx::query_as::<_, (i64, String, String, i64, Option<String>, i64)>(&sql)
            .fetch_all(&self.pool)
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string(), Some(sql)))?;
        if rows.is_empty() {
            return Err(source_error(
                &self.profile_name,
                format!("unknown table `{table}`"),
                None,
            ));
        }
        Ok(TableSchema {
            table: table.to_string(),
            table_token: table.to_string(),
            columns: rows
                .into_iter()
                .map(|(_, name, data_type, notnull, _, _)| ColumnInfo {
                    name: name.clone(),
                    name_token: name,
                    data_type,
                    nullable: notnull == 0,
                    allowed: true,
                })
                .collect(),
            limit_cap: Some(self.limit_cap),
        })
    }

    async fn query(&self, query: &CannedQuery) -> Result<Vec<LensRow>, LensError> {
        let schema = self.schema(&query.table).await?;
        let compiled = query
            .compile_to_sql_for(&schema, Dialect::Sqlite)
            .map_err(|err| {
                source_error(
                    &self.profile_name,
                    err.to_string(),
                    Some("<canned>".to_string()),
                )
            })?;
        let sql = compiled.sql.clone();
        let mut sqlx_query = sqlx::query(&compiled.sql);
        for value in compiled.binds {
            sqlx_query = bind_value(sqlx_query, value)?;
        }
        let rows = sqlx_query
            .fetch_all(&self.pool)
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string(), Some(sql.clone())))?;
        rows.iter()
            .map(|row| row_to_values(row, &schema, &self.json_text_columns))
            .collect::<Result<Vec<_>, _>>()
    }
}

fn bind_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    value: QueryValue,
) -> Result<sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>, LensError> {
    Ok(match value {
        QueryValue::String(value) => query.bind(value),
        QueryValue::I64(value) => query.bind(value),
        QueryValue::U64(value) => query.bind(i64::try_from(value).map_err(|_| {
            LensError::ConvertError(LowerError::Unsupported(
                "sqlite unsigned integer bind".to_string(),
            ))
        })?),
        QueryValue::F64(value) => query.bind(value),
        QueryValue::Bool(value) => query.bind(value),
    })
}

fn row_to_values(
    row: &SqliteRow,
    schema: &TableSchema,
    json_text_columns: &BTreeSet<String>,
) -> Result<LensRow, LensError> {
    let mut out = BTreeMap::new();
    for (index, column) in row.columns().iter().enumerate() {
        let name = column.name().to_string();
        let declared_ty = schema
            .columns
            .iter()
            .find(|candidate| candidate.name == name)
            .map(|candidate| candidate.data_type.as_str())
            .unwrap_or("");
        let value = decode_value(
            row,
            index,
            &schema.table,
            &name,
            declared_ty,
            json_text_columns,
        )?;
        out.insert(name, value);
    }
    Ok(out)
}

fn decode_value(
    row: &SqliteRow,
    index: usize,
    table_name: &str,
    column_name: &str,
    declared_ty: &str,
    json_text_columns: &BTreeSet<String>,
) -> Result<LensValue, LensError> {
    let raw = row
        .try_get_raw(index)
        .map_err(|err| decode_error("sqlite", err))?;
    if raw.is_null() {
        return Ok(LensValue::Null);
    }

    let runtime_ty = raw.type_info().name().to_ascii_uppercase();
    let declared_upper = declared_ty.to_ascii_uppercase();
    match runtime_ty.as_str() {
        "INTEGER" => {
            let value = row
                .try_get::<i64, _>(index)
                .map_err(|err| decode_error("INTEGER", err))?;
            if declared_upper.contains("BOOL") {
                return Ok(match value {
                    0 => LensValue::Bool(false),
                    1 => LensValue::Bool(true),
                    _ => LensValue::I64(value),
                });
            }
            if is_datetime_declared(&declared_upper)
                && let Some(value) = parse_datetime(&value.to_string())
            {
                return Ok(value);
            }
            Ok(LensValue::I64(value))
        }
        "REAL" => {
            let value = row
                .try_get::<f64, _>(index)
                .map_err(|err| decode_error("REAL", err))?;
            if is_datetime_declared(&declared_upper)
                && let Some(value) = parse_datetime(&value.to_string())
            {
                return Ok(value);
            }
            Ok(LensValue::F64(value))
        }
        "TEXT" => {
            let value = row
                .try_get::<String, _>(index)
                .map_err(|err| decode_error("TEXT", err))?;
            if is_datetime_declared(&declared_upper)
                && let Some(value) = parse_datetime(&value)
            {
                return Ok(value);
            }
            if json_text_columns.contains(&format!("{table_name}.{column_name}")) {
                return serde_json::from_str::<serde_json::Value>(&value)
                    .map(LensValue::Json)
                    .map_err(|err| decode_error("TEXT json", err));
            }
            Ok(LensValue::String(value))
        }
        "BLOB" => row
            .try_get::<Vec<u8>, _>(index)
            .map(|bytes| LensValue::Bytes {
                base64: base64_encode(&bytes),
                len: bytes.len(),
            })
            .map_err(|err| decode_error("BLOB", err)),
        other => Err(LensError::ConvertError(LowerError::Unsupported(
            other.to_string(),
        ))),
    }
}

fn is_datetime_declared(declared_upper: &str) -> bool {
    declared_upper.contains("DATE")
        || declared_upper.contains("TIME")
        || declared_upper.contains("TIMESTAMP")
}

fn parse_datetime(value: &str) -> Option<LensValue> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map(|value| LensValue::DateTime(value.format(&Rfc3339).expect("format rfc3339")))
        .ok()
}

fn escape_ident(ident: &str) -> String {
    format!("`{}`", ident.replace('`', "``"))
}

fn source_error(source_name: &str, detail: String, sql: Option<String>) -> LensError {
    LensError::SourceError {
        source_name: source_name.to_string(),
        detail,
        sql,
        stderr: None,
    }
}

fn decode_error(ty: &str, err: impl std::fmt::Display) -> LensError {
    LensError::ConvertError(LowerError::Decode {
        kind: "sqlite",
        detail: format!("{ty}: {err}"),
    })
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
