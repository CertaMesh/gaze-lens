use std::collections::BTreeMap;

use async_trait::async_trait;
use sqlx::mysql::{MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::{Column, ConnectOptions, Row, TypeInfo, ValueRef};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time};

use crate::errors::LensError;
use crate::profile::{Profile, SourceSpec};
use crate::source::db::query::{CannedQuery, QueryValue};
use crate::value::{LensRow, LensValue, LowerError};

use super::{ColumnInfo, DbKind, DbSource, TableSchema};

pub struct MysqlSource {
    pool: MySqlPool,
    profile_name: String,
    database: String,
    limit_cap: u32,
}

impl std::fmt::Debug for MysqlSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MysqlSource")
            .field("profile_name", &self.profile_name)
            .field("database", &self.database)
            .field("limit_cap", &self.limit_cap)
            .finish_non_exhaustive()
    }
}

impl MysqlSource {
    pub async fn connect(profile: &Profile, limit_cap: u32) -> Result<Self, LensError> {
        let SourceSpec::Mysql {
            host,
            port,
            database,
            username,
            readonly_required,
            ..
        } = &profile.source
        else {
            return Err(LensError::Profile {
                detail: format!("profile `{}` is not mysql", profile.name),
            });
        };
        if !readonly_required {
            return Err(LensError::Profile {
                detail: format!(
                    "mysql profile `{}` must require read-only mode",
                    profile.name
                ),
            });
        }
        let password = profile.resolve_password().await?;
        let options = MySqlConnectOptions::new()
            .host(host)
            .port(*port)
            .database(database)
            .username(username)
            .password(&password)
            .disable_statement_logging();
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|err| source_error(&profile.name, err.to_string(), None))?;
        sqlx::query("SET SESSION TRANSACTION READ ONLY")
            .execute(&pool)
            .await
            .map_err(|err| source_error(&profile.name, err.to_string(), None))?;
        Ok(Self {
            pool,
            profile_name: profile.name.clone(),
            database: database.clone(),
            limit_cap,
        })
    }

    #[cfg(any(test, feature = "integration-mysql"))]
    pub fn from_pool_for_tests(
        pool: MySqlPool,
        profile_name: impl Into<String>,
        database: impl Into<String>,
        limit_cap: u32,
    ) -> Self {
        Self {
            pool,
            profile_name: profile_name.into(),
            database: database.into(),
            limit_cap,
        }
    }
}

#[async_trait]
impl DbSource for MysqlSource {
    fn kind(&self) -> DbKind {
        DbKind::Mysql
    }

    fn profile_name(&self) -> &str {
        &self.profile_name
    }

    async fn list_tables(&self) -> Result<Vec<String>, LensError> {
        let rows = sqlx::query_as::<_, (String,)>(
            r#"
            SELECT CAST(TABLE_NAME AS CHAR) AS table_name
            FROM information_schema.TABLES
            WHERE TABLE_SCHEMA = ?
            ORDER BY TABLE_NAME
            "#,
        )
        .bind(&self.database)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| source_error(&self.profile_name, err.to_string(), None))?;
        Ok(rows.into_iter().map(|(table,)| table).collect())
    }

    async fn schema(&self, table: &str) -> Result<TableSchema, LensError> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            r#"
            SELECT
                CAST(COLUMN_NAME AS CHAR) AS column_name,
                CAST(DATA_TYPE AS CHAR) AS data_type,
                CAST(IS_NULLABLE AS CHAR) AS is_nullable
            FROM information_schema.COLUMNS
            WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
            ORDER BY ORDINAL_POSITION
            "#,
        )
        .bind(&self.database)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| source_error(&self.profile_name, err.to_string(), None))?;
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
                .map(|(name, data_type, nullable)| ColumnInfo {
                    name: name.clone(),
                    name_token: name,
                    data_type,
                    nullable: nullable == "YES",
                    allowed: true,
                })
                .collect(),
            limit_cap: Some(self.limit_cap),
        })
    }

    async fn query(&self, query: &CannedQuery) -> Result<Vec<LensRow>, LensError> {
        let schema = self.schema(&query.table).await?;
        let compiled = query.compile_to_sql(&schema).map_err(|err| {
            source_error(
                &self.profile_name,
                err.to_string(),
                Some("<canned>".to_string()),
            )
        })?;
        let sql = compiled.sql.clone();
        let mut sqlx_query = sqlx::query(&compiled.sql);
        for value in compiled.binds {
            sqlx_query = bind_value(sqlx_query, value);
        }
        let rows = sqlx_query
            .fetch_all(&self.pool)
            .await
            .map_err(|err| source_error(&self.profile_name, err.to_string(), Some(sql.clone())))?;
        rows.iter()
            .map(row_to_values)
            .collect::<Result<Vec<_>, _>>()
    }
}

fn bind_value<'q>(
    query: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    value: QueryValue,
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match value {
        QueryValue::String(value) => query.bind(value),
        QueryValue::I64(value) => query.bind(value),
        QueryValue::U64(value) => query.bind(value),
        QueryValue::F64(value) => query.bind(value),
        QueryValue::Bool(value) => query.bind(value),
    }
}

fn row_to_values(row: &MySqlRow) -> Result<LensRow, LensError> {
    let mut out = BTreeMap::new();
    for (index, column) in row.columns().iter().enumerate() {
        let name = column.name().to_string();
        let value = decode_value(row, index, column.type_info().name())?;
        out.insert(name, value);
    }
    Ok(out)
}

fn decode_value(row: &MySqlRow, index: usize, ty: &str) -> Result<LensValue, LensError> {
    let raw = row
        .try_get_raw(index)
        .map_err(|err| decode_error(ty, err))?;
    if raw.is_null() {
        return Ok(LensValue::Null);
    }

    let upper = ty.to_ascii_uppercase();
    match upper.as_str() {
        "BOOL" | "BOOLEAN" | "TINYINT(1)" => row
            .try_get::<bool, _>(index)
            .map(LensValue::Bool)
            .map_err(|err| decode_error(ty, err)),
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT" => row
            .try_get::<i64, _>(index)
            .map(LensValue::I64)
            .map_err(|err| decode_error(ty, err)),
        "TINYINT UNSIGNED" | "SMALLINT UNSIGNED" | "MEDIUMINT UNSIGNED" | "INT UNSIGNED"
        | "INTEGER UNSIGNED" | "BIGINT UNSIGNED" => row
            .try_get::<u64, _>(index)
            .map(LensValue::U64)
            .map_err(|err| decode_error(ty, err)),
        "FLOAT" | "DOUBLE" => row
            .try_get::<f64, _>(index)
            .map(LensValue::F64)
            .map_err(|err| decode_error(ty, err)),
        "DECIMAL" | "NUMERIC" => row
            .try_get::<String, _>(index)
            .map(|value| LensValue::Decimal {
                precision: decimal_precision(&value),
                scale: decimal_scale(&value),
                value,
            })
            .map_err(|err| decode_error(ty, err)),
        "CHAR" | "VARCHAR" | "TEXT" | "TINYTEXT" | "MEDIUMTEXT" | "LONGTEXT" => row
            .try_get::<String, _>(index)
            .map(LensValue::String)
            .map_err(|err| decode_error(ty, err)),
        "BINARY" | "VARBINARY" | "BLOB" | "TINYBLOB" | "MEDIUMBLOB" | "LONGBLOB" => row
            .try_get::<Vec<u8>, _>(index)
            .map(|bytes| LensValue::Bytes {
                base64: base64_encode(&bytes),
                len: bytes.len(),
            })
            .map_err(|err| decode_error(ty, err)),
        "DATE" => row
            .try_get::<Date, _>(index)
            .and_then(|value| format_date(value).map_err(sqlx::Error::Decode))
            .map(LensValue::DateTime)
            .map_err(|err| decode_error(ty, err)),
        "DATETIME" => row
            .try_get::<PrimitiveDateTime, _>(index)
            .and_then(|value| format_primitive_datetime(value).map_err(sqlx::Error::Decode))
            .map(LensValue::DateTime)
            .map_err(|err| decode_error(ty, err)),
        "TIMESTAMP" => row
            .try_get::<OffsetDateTime, _>(index)
            .and_then(|value| format_offset_datetime(value).map_err(sqlx::Error::Decode))
            .map(LensValue::DateTime)
            .map_err(|err| decode_error(ty, err)),
        "JSON" => row
            .try_get::<serde_json::Value, _>(index)
            .map(LensValue::Json)
            .map_err(|err| decode_error(ty, err)),
        other => Err(LensError::ConvertError(LowerError::Unsupported(
            other.to_string(),
        ))),
    }
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
        kind: "mysql",
        detail: format!("{ty}: {err}"),
    })
}

fn decimal_precision(value: &str) -> u8 {
    value
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .count()
        .min(u8::MAX as usize) as u8
}

fn decimal_scale(value: &str) -> u8 {
    value
        .split_once('.')
        .map(|(_, scale)| scale.chars().take_while(|ch| ch.is_ascii_digit()).count())
        .unwrap_or(0)
        .min(u8::MAX as usize) as u8
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

fn format_date(value: Date) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    format_offset_datetime(value.with_time(Time::MIDNIGHT).assume_utc())
}

fn format_primitive_datetime(
    value: PrimitiveDateTime,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    format_offset_datetime(value.assume_utc())
}

fn format_offset_datetime(
    value: OffsetDateTime,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    Ok(value.format(&Rfc3339)?)
}

#[cfg(test)]
mod tests {
    use time::{Date, Month, PrimitiveDateTime, Time};

    #[test]
    fn date_formats_as_midnight_utc_rfc3339() {
        let date = Date::from_calendar_date(2026, Month::April, 26).expect("date");
        assert_eq!(
            super::format_date(date).expect("format"),
            "2026-04-26T00:00:00Z"
        );
    }

    #[test]
    fn datetime_formats_as_utc_rfc3339() {
        let date = Date::from_calendar_date(2026, Month::April, 26).expect("date");
        let time = Time::from_hms(22, 30, 15).expect("time");
        assert_eq!(
            super::format_primitive_datetime(PrimitiveDateTime::new(date, time)).expect("format"),
            "2026-04-26T22:30:15Z"
        );
    }
}
