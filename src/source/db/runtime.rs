use std::sync::Arc;

use async_trait::async_trait;

use crate::errors::LensError;
use crate::profile::{Profile, SourceSpec};
use crate::source::db::mysql::MysqlSource;
use crate::source::db::postgres::PostgresSource;
use crate::source::db::query::{CannedQuery, TableSchema};
use crate::source::db::sqlite::SqliteSource;
use crate::source::db::{DbKind, DbSource};
use crate::source::ssh_tunnel::{SshTunnel, TunnelSpec};
use crate::value::LensRow;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbRuntimePlan {
    pub connect_host: String,
    pub connect_port: u16,
    pub tunnel: Option<TunnelSpec>,
}

pub fn runtime_plan(profile: &Profile) -> Result<DbRuntimePlan, LensError> {
    match &profile.source {
        SourceSpec::Mysql {
            host,
            port,
            ssh_host,
            local_port,
            ..
        }
        | SourceSpec::Postgres {
            host,
            port,
            ssh_host,
            local_port,
            ..
        } => db_runtime_plan(&profile.name, host, *port, ssh_host.as_ref(), *local_port),
        SourceSpec::Sqlite { .. } => Ok(DbRuntimePlan {
            connect_host: String::new(),
            connect_port: 0,
            tunnel: None,
        }),
        SourceSpec::SshLog { .. } => Err(LensError::Profile {
            detail: format!("profile `{}` is not a database source", profile.name),
        }),
    }
}

pub async fn connect_db_source(
    profile: &Profile,
    limit_cap: u32,
) -> Result<Arc<dyn DbSource>, LensError> {
    let plan = runtime_plan(profile)?;
    let tunnel = match &plan.tunnel {
        Some(spec) => Some(SshTunnel::open(spec).map_err(|err| LensError::SourceError {
            source_name: profile.name.clone(),
            detail: format!("ssh tunnel failed: {err}"),
            sql: None,
            stderr: None,
        })?),
        None => None,
    };

    let inner: Arc<dyn DbSource> = match &profile.source {
        SourceSpec::Mysql { .. } => Arc::new(
            MysqlSource::connect_with_target(
                profile,
                limit_cap,
                &plan.connect_host,
                plan.connect_port,
            )
            .await?,
        ),
        SourceSpec::Postgres { .. } => Arc::new(
            PostgresSource::connect_with_target(
                profile,
                limit_cap,
                &plan.connect_host,
                plan.connect_port,
            )
            .await?,
        ),
        SourceSpec::Sqlite { .. } => Arc::new(SqliteSource::connect(profile, limit_cap).await?),
        SourceSpec::SshLog { .. } => {
            return Err(LensError::Profile {
                detail: format!("profile `{}` is not a database source", profile.name),
            });
        }
    };

    Ok(Arc::new(RuntimeDbSource {
        inner,
        _tunnel: tunnel,
    }))
}

fn db_runtime_plan(
    profile_name: &str,
    remote_host: &str,
    remote_port: u16,
    ssh_host: Option<&String>,
    local_port: Option<u16>,
) -> Result<DbRuntimePlan, LensError> {
    match (ssh_host, local_port) {
        (Some(ssh_host), Some(local_port)) => Ok(DbRuntimePlan {
            connect_host: "127.0.0.1".to_string(),
            connect_port: local_port,
            tunnel: Some(TunnelSpec {
                ssh_host: ssh_host.clone(),
                local_port,
                remote_host: remote_host.to_string(),
                remote_port,
            }),
        }),
        (None, None) => Ok(DbRuntimePlan {
            connect_host: remote_host.to_string(),
            connect_port: remote_port,
            tunnel: None,
        }),
        _ => Err(LensError::Profile {
            detail: format!(
                "profile `{profile_name}` db tunnel requires both `ssh_host` and `local_port`"
            ),
        }),
    }
}

struct RuntimeDbSource {
    inner: Arc<dyn DbSource>,
    _tunnel: Option<SshTunnel>,
}

#[async_trait]
impl DbSource for RuntimeDbSource {
    fn kind(&self) -> DbKind {
        self.inner.kind()
    }

    fn profile_name(&self) -> &str {
        self.inner.profile_name()
    }

    async fn list_tables(&self) -> Result<Vec<String>, LensError> {
        self.inner.list_tables().await
    }

    async fn schema(&self, table: &str) -> Result<TableSchema, LensError> {
        self.inner.schema(table).await
    }

    async fn query(&self, query: &CannedQuery) -> Result<Vec<LensRow>, LensError> {
        self.inner.query(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::SecretSpec;

    fn mysql_profile(ssh_host: Option<&str>, local_port: Option<u16>) -> Profile {
        Profile {
            name: "prod".to_string(),
            source: SourceSpec::Mysql {
                host: "db.internal".to_string(),
                port: 3306,
                database: "app".to_string(),
                username: "readonly".to_string(),
                password_env: Some("DB_PASSWORD".to_string()),
                secret: None::<SecretSpec>,
                ssh_host: ssh_host.map(str::to_string),
                local_port,
                readonly_required: true,
            },
            discovered_from_ssh_host: None,
            discovered_from_path: None,
            discovered_at: None,
            discovered_ssh_host_key_fingerprint: None,
            credential_class: None,
            policy: None,
            schema_tokenize: None,
            schema_allowlist: None,
            snapshot_retention_days: None,
            auto_purge: crate::session::maintenance::AutoPurge::Warn,
        }
    }

    #[test]
    fn tunneled_db_plan_uses_local_target_and_remote_tunnel_endpoint() {
        let profile = mysql_profile(Some("deploy@app01"), Some(13306));

        let plan = runtime_plan(&profile).expect("runtime plan");

        assert_eq!(plan.connect_host, "127.0.0.1");
        assert_eq!(plan.connect_port, 13306);
        assert_eq!(
            plan.tunnel,
            Some(TunnelSpec {
                ssh_host: "deploy@app01".to_string(),
                local_port: 13306,
                remote_host: "db.internal".to_string(),
                remote_port: 3306,
            })
        );
    }

    #[test]
    fn direct_db_plan_uses_profile_target() {
        let profile = mysql_profile(None, None);

        let plan = runtime_plan(&profile).expect("runtime plan");

        assert_eq!(plan.connect_host, "db.internal");
        assert_eq!(plan.connect_port, 3306);
        assert_eq!(plan.tunnel, None);
    }

    #[test]
    fn partial_db_tunnel_config_is_rejected() {
        let profile = mysql_profile(Some("deploy@app01"), None);

        let err = runtime_plan(&profile).expect_err("partial tunnel must fail");

        assert!(
            err.to_string()
                .contains("requires both `ssh_host` and `local_port`"),
            "{err}"
        );
    }
}
