use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use super::{ColumnInfo, TableSchema};

pub const SCHEMA_METADATA_SOURCE_CLASS: &str = "schema_metadata";

#[derive(Debug, Default)]
pub struct SchemaTokenizer {
    inner: Mutex<SchemaTokenizerInner>,
}

#[derive(Debug, Default)]
struct SchemaTokenizerInner {
    table_tokens: HashMap<String, String>,
    column_tokens: HashMap<String, String>,
}

impl SchemaTokenizer {
    pub fn tokenize_table_schema(
        &self,
        schema: &TableSchema,
        profile_allowlist: Option<&[String]>,
    ) -> TableSchema {
        let allowlist = schema_allowlist(profile_allowlist);
        let table_token = self.token_for("TABLE", &schema.table, &allowlist);
        TableSchema {
            table: schema.table.clone(),
            table_token,
            columns: schema
                .columns
                .iter()
                .map(|column| ColumnInfo {
                    name: column.name.clone(),
                    name_token: self.token_for("COL", &column.name, &allowlist),
                    data_type: column.data_type.clone(),
                    nullable: column.nullable,
                    allowed: allowlist.contains(&column.name) || column.allowed,
                })
                .collect(),
            limit_cap: schema.limit_cap,
        }
    }

    pub fn tokenize_table_names(
        &self,
        tables: &[String],
        profile_allowlist: Option<&[String]>,
    ) -> Vec<String> {
        let allowlist = schema_allowlist(profile_allowlist);
        tables
            .iter()
            .map(|table| self.token_for("TABLE", table, &allowlist))
            .collect()
    }

    fn token_for(&self, prefix: &str, value: &str, allowlist: &BTreeSet<String>) -> String {
        if allowlist.contains(value) || is_default_allowed_name(value) {
            return value.to_string();
        }

        let mut inner = self.inner.lock().expect("schema tokenizer lock");
        let map = if prefix == "TABLE" {
            &mut inner.table_tokens
        } else {
            &mut inner.column_tokens
        };
        if let Some(token) = map.get(value) {
            return token.clone();
        }
        let token = format!("<{prefix}_{:03}>", map.len() + 1);
        map.insert(value.to_string(), token.clone());
        token
    }
}

pub fn schema_allowlist(profile_allowlist: Option<&[String]>) -> BTreeSet<String> {
    let mut allowlist = default_schema_allowlist()
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    if let Some(profile_allowlist) = profile_allowlist {
        allowlist.extend(profile_allowlist.iter().cloned());
    }
    allowlist
}

pub fn default_schema_allowlist() -> Vec<&'static str> {
    vec![
        "id",
        "created_at",
        "updated_at",
        "deleted_at",
        "status",
        "type",
    ]
}

fn is_default_allowed_name(value: &str) -> bool {
    value.ends_with("_at") || default_schema_allowlist().contains(&value)
}
