use std::collections::BTreeMap;

use async_trait::async_trait;
use log::LevelFilter;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions, PgRow};
use sqlx::types::BigDecimal;
use sqlx::{Column, ConnectOptions, Row, TypeInfo, ValueRef};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime, PrimitiveDateTime, Time};

use crate::errors::LensError;
use crate::profile::{Profile, SourceSpec};
use crate::source::db::query::{CannedQuery, Dialect, QueryValue};
use crate::value::{LensRow, LensValue, LowerError};

use super::{ColumnInfo, DbKind, DbSource, TableSchema};

pub struct PostgresSource {
    pool: PgPool,
    profile_name: String,
    limit_cap: u32,
}

impl std::fmt::Debug for PostgresSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresSource")
            .field("profile_name", &self.profile_name)
            .field("limit_cap", &self.limit_cap)
            .finish_non_exhaustive()
    }
}

impl PostgresSource {
    pub async fn connect(profile: &Profile, limit_cap: u32) -> Result<Self, LensError> {
        let SourceSpec::Postgres {
            host,
            port,
            database,
            username,
            readonly_required,
            ..
        } = &profile.source
        else {
            return Err(LensError::Profile {
                detail: format!("profile `{}` is not postgres", profile.name),
            });
        };
        if !readonly_required {
            return Err(LensError::Profile {
                detail: format!(
                    "postgres profile `{}` must require read-only mode",
                    profile.name
                ),
            });
        }
        let password = profile.resolve_password().await?;
        let options = PgConnectOptions::new()
            .host(host)
            .port(*port)
            .database(database)
            .username(username)
            .password(&password)
            .application_name("gaze-lens")
            .log_statements(LevelFilter::Off)
            .disable_statement_logging();
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|err| source_error(&profile.name, err.to_string(), None))?;
        sqlx::query("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
            .execute(&pool)
            .await
            .map_err(|err| source_error(&profile.name, err.to_string(), None))?;
        Ok(Self {
            pool,
            profile_name: profile.name.clone(),
            limit_cap,
        })
    }

    #[cfg(any(test, feature = "integration-postgres"))]
    pub fn from_pool_for_tests(
        pool: PgPool,
        profile_name: impl Into<String>,
        limit_cap: u32,
    ) -> Self {
        Self {
            pool,
            profile_name: profile_name.into(),
            limit_cap,
        }
    }
}

#[async_trait]
impl DbSource for PostgresSource {
    fn kind(&self) -> DbKind {
        DbKind::Postgres
    }

    fn profile_name(&self) -> &str {
        &self.profile_name
    }

    async fn list_tables(&self) -> Result<Vec<String>, LensError> {
        let rows = sqlx::query_as::<_, (String,)>(
            r#"
            SELECT table_name
            FROM information_schema.tables
            WHERE table_schema = current_schema()
              AND table_type = 'BASE TABLE'
            ORDER BY table_name
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|err| source_error(&self.profile_name, err.to_string(), None))?;
        Ok(rows.into_iter().map(|(table,)| table).collect())
    }

    async fn schema(&self, table: &str) -> Result<TableSchema, LensError> {
        let rows = sqlx::query_as::<_, (String, String, String)>(
            r#"
            SELECT column_name, data_type, is_nullable
            FROM information_schema.columns
            WHERE table_schema = current_schema() AND table_name = $1
            ORDER BY ordinal_position
            "#,
        )
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
        let compiled = query
            .compile_to_sql_for(&schema, Dialect::Postgres)
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
            .map(|row| row_to_values(row, &schema))
            .collect::<Result<Vec<_>, _>>()
    }
}

fn bind_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: QueryValue,
) -> Result<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>, LensError> {
    Ok(match value {
        QueryValue::String(value) => query.bind(value),
        QueryValue::I64(value) => query.bind(value),
        QueryValue::U64(value) => query.bind(i64::try_from(value).map_err(|_| {
            LensError::ConvertError(LowerError::Unsupported(
                "postgres unsigned integer bind".to_string(),
            ))
        })?),
        QueryValue::F64(value) => query.bind(value),
        QueryValue::Bool(value) => query.bind(value),
    })
}

fn row_to_values(row: &PgRow, schema: &TableSchema) -> Result<LensRow, LensError> {
    let mut out = BTreeMap::new();
    for (index, column) in row.columns().iter().enumerate() {
        let name = column.name().to_string();
        let data_type = schema
            .columns
            .iter()
            .find(|candidate| candidate.name == name)
            .map(|candidate| candidate.data_type.as_str())
            .unwrap_or(column.type_info().name());
        let value = decode_value(row, index, column.type_info().name(), data_type)?;
        out.insert(name, value);
    }
    Ok(out)
}

fn decode_value(
    row: &PgRow,
    index: usize,
    runtime_ty: &str,
    declared_ty: &str,
) -> Result<LensValue, LensError> {
    let raw = row
        .try_get_raw(index)
        .map_err(|err| decode_error(runtime_ty, err))?;
    if raw.is_null() {
        return Ok(LensValue::Null);
    }

    let upper = runtime_ty.to_ascii_uppercase();
    if upper.starts_with('_') || declared_ty.eq_ignore_ascii_case("ARRAY") {
        return Err(LensError::ConvertError(LowerError::Unsupported(
            "array".to_string(),
        )));
    }

    match upper.as_str() {
        "BOOL" => row
            .try_get::<bool, _>(index)
            .map(LensValue::Bool)
            .map_err(|err| decode_error(runtime_ty, err)),
        "INT2" | "INT4" | "INT8" => row
            .try_get::<i64, _>(index)
            .map(LensValue::I64)
            .map_err(|err| decode_error(runtime_ty, err)),
        "FLOAT4" | "FLOAT8" => row
            .try_get::<f64, _>(index)
            .map(LensValue::F64)
            .map_err(|err| decode_error(runtime_ty, err)),
        "NUMERIC" => row
            .try_get::<BigDecimal, _>(index)
            .map(decimal_value)
            .map_err(|err| decode_error(runtime_ty, err)),
        "VARCHAR" | "TEXT" | "BPCHAR" | "CHAR" | "NAME" => row
            .try_get::<String, _>(index)
            .map(LensValue::String)
            .map_err(|err| decode_error(runtime_ty, err)),
        "BYTEA" => row
            .try_get::<Vec<u8>, _>(index)
            .map(|bytes| LensValue::Bytes {
                base64: base64_encode(&bytes),
                len: bytes.len(),
            })
            .map_err(|err| decode_error(runtime_ty, err)),
        "DATE" => row
            .try_get::<Date, _>(index)
            .and_then(|value| format_date(value).map_err(sqlx::Error::Decode))
            .map(LensValue::DateTime)
            .map_err(|err| decode_error(runtime_ty, err)),
        "TIME" => row
            .try_get::<Time, _>(index)
            .and_then(|value| format_time(value).map_err(sqlx::Error::Decode))
            .map(LensValue::DateTime)
            .map_err(|err| decode_error(runtime_ty, err)),
        "TIMESTAMP" => row
            .try_get::<PrimitiveDateTime, _>(index)
            .and_then(|value| format_primitive_datetime(value).map_err(sqlx::Error::Decode))
            .map(LensValue::DateTime)
            .map_err(|err| decode_error(runtime_ty, err)),
        "TIMESTAMPTZ" => row
            .try_get::<OffsetDateTime, _>(index)
            .and_then(|value| format_offset_datetime(value).map_err(sqlx::Error::Decode))
            .map(LensValue::DateTime)
            .map_err(|err| decode_error(runtime_ty, err)),
        "UUID" => row
            .try_get::<sqlx::types::Uuid, _>(index)
            .map(|value| LensValue::Uuid(value.to_string()))
            .map_err(|err| decode_error(runtime_ty, err)),
        "JSON" | "JSONB" => row
            .try_get::<serde_json::Value, _>(index)
            .map(LensValue::Json)
            .map_err(|err| decode_error(runtime_ty, err)),
        _ if declared_ty.eq_ignore_ascii_case("USER-DEFINED") => row
            .try_get::<String, _>(index)
            .map(LensValue::String)
            .map_err(|err| decode_error(runtime_ty, err)),
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
        kind: "postgres",
        detail: format!("{ty}: {err}"),
    })
}

fn decimal_value(value: BigDecimal) -> LensValue {
    let decimal = value.to_string();
    let (mantissa, scale) = value.into_bigint_and_scale();
    let precision = mantissa
        .to_string()
        .trim_start_matches('-')
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .count()
        .min(u8::MAX as usize) as u8;
    let scale = u8::try_from(scale.max(0)).unwrap_or(u8::MAX);
    LensValue::Decimal {
        value: decimal,
        precision,
        scale,
    }
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

fn format_time(value: Time) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let date = Date::from_calendar_date(1970, time::Month::January, 1)?;
    format_offset_datetime(date.with_time(value).assume_utc())
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
    use std::str::FromStr;

    use sqlx::types::BigDecimal;

    use crate::value::LensValue;

    #[test]
    fn decimal_metadata_counts_precision_and_scale() {
        assert_eq!(
            super::decimal_value(BigDecimal::from_str("123.456").expect("decimal")),
            LensValue::Decimal {
                value: "123.456".to_string(),
                precision: 6,
                scale: 3,
            }
        );
    }

    #[test]
    fn decimal_metadata_handles_zero_scale() {
        assert_eq!(
            super::decimal_value(BigDecimal::from_str("42").expect("decimal")),
            LensValue::Decimal {
                value: "42".to_string(),
                precision: 2,
                scale: 0,
            }
        );
    }

    #[test]
    fn decimal_metadata_ignores_negative_sign_for_precision() {
        assert_eq!(
            super::decimal_value(BigDecimal::from_str("-99.9").expect("decimal")),
            LensValue::Decimal {
                value: "-99.9".to_string(),
                precision: 3,
                scale: 1,
            }
        );
    }
}
