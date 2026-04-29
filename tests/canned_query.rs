use gaze_lens::source::db::query::{
    CannedQuery, ColumnInfo, Dialect, OrderBy, OrderDir, QueryError, QueryValue, ScalarOrList,
    ScalarValue, TableSchema, WhereClause, WhereCombinator, WhereOp,
};

fn schema() -> TableSchema {
    TableSchema {
        table: "users".to_string(),
        table_token: "<TABLE_001>".to_string(),
        limit_cap: Some(50),
        columns: vec![
            column("id", "bigint"),
            column("email", "varchar"),
            column("age", "int"),
            column("created_at", "datetime"),
            column("select_count", "int"),
            column("user_select", "varchar"),
        ],
    }
}

fn column(name: &str, data_type: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.to_string(),
        name_token: name.to_string(),
        data_type: data_type.to_string(),
        nullable: false,
        allowed: true,
    }
}

#[test]
fn roundtrip_every_operator_and_or_order_path() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: Some(vec!["id".to_string(), "email".to_string()]),
        r#where: Some(vec![
            clause("id", WhereOp::Eq, scalar(ScalarValue::I64(1))),
            clause("id", WhereOp::Ne, scalar(ScalarValue::I64(2))),
            clause("age", WhereOp::Gt, scalar(ScalarValue::I64(18))),
            clause("age", WhereOp::Gte, scalar(ScalarValue::I64(21))),
            clause("age", WhereOp::Lt, scalar(ScalarValue::I64(70))),
            clause("age", WhereOp::Lte, scalar(ScalarValue::I64(65))),
            clause(
                "id",
                WhereOp::In,
                Some(ScalarOrList::List(vec![
                    ScalarValue::I64(3),
                    ScalarValue::I64(4),
                ])),
            ),
            clause(
                "email",
                WhereOp::Like,
                scalar(ScalarValue::String("%@example.com".to_string())),
            ),
            clause("created_at", WhereOp::IsNull, None),
            clause("created_at", WhereOp::IsNotNull, None),
        ]),
        where_combinator: Some(WhereCombinator::Or),
        order_by: Some(vec![OrderBy {
            col: "created_at".to_string(),
            dir: OrderDir::Desc,
        }]),
        limit: Some(10),
    };

    let compiled = query.compile_to_sql(&schema()).expect("compile");

    assert_eq!(
        compiled.sql,
        "SELECT `id`, `email` FROM `users` WHERE `id` = ? OR `id` != ? OR `age` > ? OR `age` >= ? OR `age` < ? OR `age` <= ? OR `id` IN (?, ?) OR `email` LIKE ? OR `created_at` IS NULL OR `created_at` IS NOT NULL ORDER BY `created_at` DESC LIMIT ?"
    );
    assert_eq!(
        compiled.binds,
        vec![
            QueryValue::I64(1),
            QueryValue::I64(2),
            QueryValue::I64(18),
            QueryValue::I64(21),
            QueryValue::I64(70),
            QueryValue::I64(65),
            QueryValue::I64(3),
            QueryValue::I64(4),
            QueryValue::String("%@example.com".to_string()),
            QueryValue::U64(10),
        ]
    );
}

#[test]
fn default_combinator_is_and() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: None,
        r#where: Some(vec![
            clause("id", WhereOp::Eq, scalar(ScalarValue::I64(1))),
            clause(
                "email",
                WhereOp::Like,
                scalar(ScalarValue::String("%@example.com".to_string())),
            ),
        ]),
        where_combinator: None,
        order_by: None,
        limit: Some(5),
    };

    let compiled = query.compile_to_sql(&schema()).expect("compile");

    assert!(compiled.sql.contains("`id` = ? AND `email` LIKE ?"));
}

#[test]
fn rejects_column_not_in_schema() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: Some(vec!["unknown".to_string()]),
        r#where: None,
        where_combinator: None,
        order_by: None,
        limit: None,
    };

    assert_eq!(
        query.compile_to_sql(&schema()).expect_err("unknown"),
        QueryError::UnknownColumn("unknown".to_string())
    );
}

#[test]
fn rejects_operator_type_mismatch() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: None,
        r#where: Some(vec![clause(
            "age",
            WhereOp::Like,
            scalar(ScalarValue::String("%42%".to_string())),
        )]),
        where_combinator: None,
        order_by: None,
        limit: None,
    };

    assert!(matches!(
        query.compile_to_sql(&schema()),
        Err(QueryError::OperatorTypeMismatch { col, .. }) if col == "age"
    ));
}

#[test]
fn rejects_in_with_non_list_value() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: None,
        r#where: Some(vec![clause("id", WhereOp::In, scalar(ScalarValue::I64(1)))]),
        where_combinator: None,
        order_by: None,
        limit: None,
    };

    assert_eq!(
        query.compile_to_sql(&schema()).expect_err("list"),
        QueryError::OperatorRequiresList(WhereOp::In)
    );
}

#[test]
fn rejects_malicious_column_before_sql_construction() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: None,
        r#where: Some(vec![clause(
            "1; DROP TABLE users; --",
            WhereOp::Eq,
            scalar(ScalarValue::I64(1)),
        )]),
        where_combinator: None,
        order_by: None,
        limit: None,
    };

    assert!(matches!(
        query.compile_to_sql(&schema()),
        Err(QueryError::UnsafeColumn(_))
    ));
}

#[test]
fn keyword_fragments_inside_identifiers_are_allowed() {
    for column_name in ["select_count", "user_select"] {
        let query = CannedQuery {
            table: "users".to_string(),
            columns: Some(vec![column_name.to_string()]),
            r#where: None,
            where_combinator: None,
            order_by: None,
            limit: Some(1),
        };

        let compiled = query.compile_to_sql(&schema()).expect("compile");

        assert_eq!(compiled.sql, format!("SELECT `{column_name}` FROM `users` LIMIT ?"));
    }
}

#[test]
fn whole_identifier_sql_keywords_are_rejected_case_insensitively() {
    for column_name in ["select", "SELECT", "Select"] {
        let query = CannedQuery {
            table: "users".to_string(),
            columns: Some(vec![column_name.to_string()]),
            r#where: None,
            where_combinator: None,
            order_by: None,
            limit: None,
        };

        assert_eq!(
            query.compile_to_sql(&schema()).expect_err("keyword"),
            QueryError::UnsafeColumn(column_name.to_string())
        );
    }
}

#[test]
fn missing_column_allowed_field_deserializes_default_deny() {
    let column: ColumnInfo = serde_json::from_value(serde_json::json!({
        "name_token": "email",
        "data_type": "varchar",
        "nullable": false
    }))
    .expect("column info");

    assert!(!column.allowed);
}

#[test]
fn limit_cap_clamps_to_schema_limit() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: Some(vec!["id".to_string()]),
        r#where: None,
        where_combinator: None,
        order_by: None,
        limit: Some(500),
    };

    let compiled = query.compile_to_sql(&schema()).expect("compile");

    assert_eq!(compiled.sql, "SELECT `id` FROM `users` LIMIT ?");
    assert_eq!(compiled.binds, vec![QueryValue::U64(50)]);
}

#[test]
fn postgres_dialect_uses_numbered_placeholders_and_quoted_idents() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: Some(vec!["id".to_string(), "email".to_string()]),
        r#where: Some(vec![
            clause("id", WhereOp::Eq, scalar(ScalarValue::I64(1))),
            clause(
                "email",
                WhereOp::Like,
                scalar(ScalarValue::String("%@example.com".to_string())),
            ),
        ]),
        where_combinator: Some(WhereCombinator::And),
        order_by: Some(vec![OrderBy {
            col: "created_at".to_string(),
            dir: OrderDir::Asc,
        }]),
        limit: Some(10),
    };

    let compiled = query
        .compile_to_sql_for(&schema(), Dialect::Postgres)
        .expect("compile");

    assert_eq!(
        compiled.sql,
        "SELECT \"id\", \"email\" FROM \"users\" WHERE \"id\" = $1 AND \"email\" LIKE $2 ORDER BY \"created_at\" ASC LIMIT $3"
    );
    assert_eq!(
        compiled.binds,
        vec![
            QueryValue::I64(1),
            QueryValue::String("%@example.com".to_string()),
            QueryValue::U64(10),
        ]
    );
}

#[test]
fn sqlite_dialect_keeps_question_placeholders_with_sqlite_safe_idents() {
    let query = CannedQuery {
        table: "users".to_string(),
        columns: Some(vec!["id".to_string()]),
        r#where: Some(vec![clause(
            "age",
            WhereOp::Gt,
            scalar(ScalarValue::I64(18)),
        )]),
        where_combinator: None,
        order_by: None,
        limit: Some(1),
    };

    let compiled = query
        .compile_to_sql_for(&schema(), Dialect::Sqlite)
        .expect("compile");

    assert_eq!(
        compiled.sql,
        "SELECT `id` FROM `users` WHERE `age` > ? LIMIT ?"
    );
    assert_eq!(
        compiled.binds,
        vec![QueryValue::I64(18), QueryValue::U64(1)]
    );
}

fn clause(col: &str, op: WhereOp, val: Option<ScalarOrList>) -> WhereClause {
    WhereClause {
        col: col.to_string(),
        op,
        val,
    }
}

fn scalar(value: ScalarValue) -> Option<ScalarOrList> {
    Some(ScalarOrList::Scalar(value))
}
