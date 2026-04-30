use gaze_lens::errors::LensError;
use gaze_lens::profile::{Profile, SourceSpec};
use gaze_lens::session::maintenance::AutoPurge;
use gaze_lens::source::db::postgres::PostgresSource;

fn postgres_profile(readonly_required: bool, password_env: &str) -> Profile {
    Profile {
        name: "prod-pg".to_string(),
        policy: None,
        schema_allowlist: None,
        snapshot_retention_days: None,
        auto_purge: AutoPurge::Off,
        source: SourceSpec::Postgres {
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "app".to_string(),
            username: "app".to_string(),
            password_env: Some(password_env.to_string()),
            secret: None,
            ssh_host: None,
            local_port: None,
            readonly_required,
        },
    }
}

#[tokio::test]
async fn refuses_postgres_profile_without_readonly_required() {
    let profile = postgres_profile(false, "GAZE_LENS_POSTGRES_TEST_PASSWORD_UNUSED");

    let err = PostgresSource::connect(&profile, 100)
        .await
        .expect_err("readonly gate should fail before connecting");

    assert!(matches!(err, LensError::Profile { .. }));
}

#[tokio::test]
async fn refuses_missing_postgres_password_env_before_connecting() {
    unsafe {
        std::env::remove_var("GAZE_LENS_POSTGRES_TEST_PASSWORD_MISSING");
    }
    let profile = postgres_profile(true, "GAZE_LENS_POSTGRES_TEST_PASSWORD_MISSING");

    let err = PostgresSource::connect(&profile, 100)
        .await
        .expect_err("missing env should fail before connecting");

    assert!(matches!(
        err,
        LensError::ProfileEnvMissing { ref env } if env == "GAZE_LENS_POSTGRES_TEST_PASSWORD_MISSING"
    ));
}

// Postgres end-to-end coverage is intentionally behind an opt-in feature
// because the standard test run must not require Docker or a local database.
// It should exercise supported scalar types, NULL preservation, bytea, uuid,
// json/jsonb, timestamp formatting, and explicit ARRAY rejection.
#[cfg(feature = "integration-postgres")]
mod integration {
    use gaze_lens::source::db::DbSource;
    use gaze_lens::source::db::query::CannedQuery;
    use gaze_lens::value::LensValue;
    use sqlx::postgres::PgPoolOptions;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres;

    use super::*;

    const PASSWORD_ENV: &str = "GAZE_LENS_POSTGRES_TESTCONTAINER_PASSWORD";
    const PASSWORD: &str = "postgres";

    #[tokio::test]
    async fn postgres_testcontainer_smoke_decodes_supported_scalars_and_rejects_arrays() {
        let node = postgres::Postgres::default()
            .with_password(PASSWORD)
            .start()
            .await
            .expect("start postgres container");
        let host = node.get_host().await.expect("host").to_string();
        let port = node.get_host_port_ipv4(5432).await.expect("port");
        let dsn = format!("postgres://postgres:{PASSWORD}@{host}:{port}/postgres");

        let setup_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&dsn)
            .await
            .expect("setup pool");
        create_fixture(&setup_pool).await;
        setup_pool.close().await;

        unsafe {
            std::env::set_var(PASSWORD_ENV, PASSWORD);
        }
        let profile = Profile {
            name: "pg-smoke".to_string(),
            policy: None,
            schema_allowlist: None,
            snapshot_retention_days: None,
            auto_purge: AutoPurge::Off,
            source: SourceSpec::Postgres {
                host,
                port,
                database: "postgres".to_string(),
                username: "postgres".to_string(),
                password_env: Some(PASSWORD_ENV.to_string()),
                secret: None,
                ssh_host: None,
                local_port: None,
                readonly_required: true,
            },
        };
        let source = PostgresSource::connect(&profile, 100)
            .await
            .expect("source connect");

        let rows = source
            .query(&CannedQuery {
                profile: "test".to_string(),
                table: "lens_smoke".to_string(),
                columns: Some(vec![
                    "numeric_value".to_string(),
                    "uuid_value".to_string(),
                    "json_value".to_string(),
                    "jsonb_value".to_string(),
                    "bytea_value".to_string(),
                    "timestamptz_value".to_string(),
                    "integer_value".to_string(),
                    "nullable_text".to_string(),
                ]),
                r#where: None,
                where_combinator: None,
                order_by: None,
                limit: Some(1),
            })
            .await
            .expect("query scalar columns");

        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            row["numeric_value"],
            LensValue::Decimal {
                value: "123456789012345.67890".to_string(),
                precision: 0,
                scale: 0,
            }
        );
        assert_eq!(
            row["uuid_value"],
            LensValue::Uuid("018f3ec3-7b3a-7b24-a71d-5d34ec55acfd".to_string())
        );
        assert_eq!(
            row["json_value"],
            LensValue::Json(serde_json::json!({"email": "alice@example.com"}))
        );
        assert_eq!(
            row["jsonb_value"],
            LensValue::Json(serde_json::json!({"nested": [1, true, "ok"]}))
        );
        assert_eq!(
            row["bytea_value"],
            LensValue::Bytes {
                base64: "AQIDBA==".to_string(),
                len: 4,
            }
        );
        assert_eq!(
            row["timestamptz_value"],
            LensValue::DateTime("2026-04-26T18:30:15Z".to_string())
        );
        assert_eq!(row["integer_value"], LensValue::I64(42));
        assert_eq!(row["nullable_text"], LensValue::Null);

        let err = source
            .query(&CannedQuery {
                profile: "test".to_string(),
                table: "lens_smoke".to_string(),
                columns: Some(vec!["int_array".to_string()]),
                r#where: None,
                where_combinator: None,
                order_by: None,
                limit: Some(1),
            })
            .await
            .expect_err("array column should be rejected");

        assert!(matches!(err, LensError::ConvertError(_)));
    }

    async fn create_fixture(pool: &sqlx::PgPool) {
        sqlx::query(
            r#"
            CREATE TABLE lens_smoke (
                numeric_value NUMERIC(20,5) NOT NULL,
                uuid_value UUID NOT NULL,
                json_value JSON NOT NULL,
                jsonb_value JSONB NOT NULL,
                bytea_value BYTEA NOT NULL,
                timestamptz_value TIMESTAMPTZ NOT NULL,
                integer_value INTEGER NOT NULL,
                nullable_text TEXT NULL,
                int_array INTEGER[] NOT NULL
            )
            "#,
        )
        .execute(pool)
        .await
        .expect("create fixture table");

        sqlx::query(
            r#"
            INSERT INTO lens_smoke (
                numeric_value,
                uuid_value,
                json_value,
                jsonb_value,
                bytea_value,
                timestamptz_value,
                integer_value,
                nullable_text,
                int_array
            ) VALUES (
                123456789012345.67890,
                '018f3ec3-7b3a-7b24-a71d-5d34ec55acfd',
                '{"email":"alice@example.com"}',
                '{"nested":[1,true,"ok"]}',
                decode('01020304', 'hex'),
                TIMESTAMPTZ '2026-04-26 20:30:15+02',
                42,
                NULL,
                ARRAY[1,2,3]
            )
            "#,
        )
        .execute(pool)
        .await
        .expect("insert fixture row");
    }
}
