//! Shared search-query expression helpers.
//!
//! The helpers in this module build small SeaQuery expressions for common
//! search behavior used by Aster repositories: escaped `LIKE` patterns,
//! case-insensitive substring checks, SQLite FTS phrase queries, and MySQL
//! boolean-mode phrase queries. They do not depend on product entities and leave
//! database-specific query composition to the caller.

use sea_orm::ExprTrait;
use sea_orm::sea_query::{
    Alias, Expr, Func, IntoColumnRef, LikeExpr, Query, SimpleExpr, extension::sqlite::SqliteExpr,
};

/// Escapes the escape character itself and the wildcard characters for SQL `LIKE` queries.
///
/// The pattern assumes `\` as the escape character. MySQL and PostgreSQL use it by default,
/// but SQLite has no default escape character, so callers building their own condition must
/// pair the pattern with `ESCAPE '\'` (e.g. via [`sea_orm::sea_query::LikeExpr::escape`])
/// for the pattern to mean the same thing on every backend.
pub fn escape_like_query(query: &str) -> String {
    query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Builds a case-insensitive `LIKE '%query%'` condition for a column.
///
/// The condition declares `ESCAPE '\'` explicitly so SQLite applies the same escape
/// semantics as MySQL and PostgreSQL instead of treating backslashes as literal text.
pub fn lower_like_condition(column: impl IntoColumnRef + Copy, query: &str) -> SimpleExpr {
    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for ch in query.chars() {
        match ch {
            '\\' => pattern.push_str("\\\\"),
            '%' => pattern.push_str("\\%"),
            '_' => pattern.push_str("\\_"),
            _ => pattern.extend(ch.to_lowercase()),
        }
    }
    pattern.push('%');
    Expr::expr(Func::lower(Expr::col(column))).like(LikeExpr::new(pattern).escape('\\'))
}

/// Builds a quoted SQLite FTS phrase query when the input is long enough.
pub fn sqlite_match_query(query: &str) -> Option<String> {
    if query.chars().count() < 3 {
        return None;
    }

    Some(format!("\"{}\"", query.replace('"', "\"\"")))
}

/// Builds a quoted MySQL boolean-mode phrase query when the input is safe.
pub fn mysql_boolean_mode_query(query: &str) -> Option<String> {
    if query.chars().count() < 3 || query.chars().any(|ch| !ch.is_alphanumeric()) {
        return None;
    }

    let escaped = query.replace('\\', "\\\\").replace('"', "\\\"");
    Some(format!("\"{escaped}\""))
}

/// Builds a SQLite FTS subquery condition matching row ids from an FTS table.
pub fn sqlite_fts_match_condition(
    id_column: impl IntoColumnRef + Copy,
    fts_table: &str,
    match_query: &str,
) -> SimpleExpr {
    Expr::col(id_column).in_subquery(
        Query::select()
            .expr(Expr::col(Alias::new("rowid")))
            .from(Alias::new(fts_table))
            .and_where(Expr::col(Alias::new(fts_table)).matches(Expr::val(match_query)))
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        escape_like_query, lower_like_condition, mysql_boolean_mode_query, sqlite_match_query,
    };
    use sea_orm::sea_query::{
        Alias, MysqlQueryBuilder, PostgresQueryBuilder, Query, SqliteQueryBuilder, Value,
    };

    #[derive(Copy, Clone)]
    struct NameColumn;

    impl sea_orm::sea_query::Iden for NameColumn {
        fn unquoted(&self) -> &str {
            "name"
        }
    }

    #[test]
    fn escape_like_query_escapes_wildcards() {
        assert_eq!(escape_like_query("100%_done"), "100\\%\\_done");
    }

    #[test]
    fn escape_like_query_escapes_backslash_before_wildcards() {
        // The backslash must be escaped first: replacing `%` before `\` would turn an
        // already-escaped `\%` into `\\%` (escaped backslash + live wildcard).
        assert_eq!(escape_like_query("a\\%"), "a\\\\\\%");
        assert_eq!(escape_like_query("a\\_b"), "a\\\\\\_b");
        assert_eq!(escape_like_query("a\\b"), "a\\\\b");
    }

    #[test]
    fn escape_like_query_escapes_trailing_backslash() {
        // A lone trailing backslash would otherwise escape the closing `%` added by
        // `lower_like_condition`, silently breaking the suffix match.
        assert_eq!(escape_like_query("a\\"), "a\\\\");
        assert_eq!(escape_like_query("\\"), "\\\\");
    }

    fn like_condition_parts(
        query: &str,
        builder: impl sea_orm::sea_query::QueryBuilder,
    ) -> (String, Vec<Value>) {
        let (sql, values) = Query::select()
            .column(Alias::new("name"))
            .from(Alias::new("items"))
            .and_where(lower_like_condition(NameColumn, query))
            .build(builder);
        (sql, values.0)
    }

    #[test]
    fn lower_like_condition_declares_escape_clause_on_all_backends() {
        for (sql, _) in [
            like_condition_parts("report", SqliteQueryBuilder),
            like_condition_parts("report", MysqlQueryBuilder),
            like_condition_parts("report", PostgresQueryBuilder),
        ] {
            assert!(sql.contains("ESCAPE"), "expected ESCAPE clause in: {sql}");
        }
    }

    #[test]
    fn lower_like_condition_binds_fully_escaped_pattern_on_all_backends() {
        // `a\%_B` must arrive at the database as one literal `a\%_` prefix followed by a
        // lowercased `b`, with every metacharacter (including the backslash itself) escaped.
        let expected = vec![Value::String(Some("%a\\\\\\%\\_b%".to_string()))];
        for (sql, values) in [
            like_condition_parts("a\\%_B", SqliteQueryBuilder),
            like_condition_parts("a\\%_B", MysqlQueryBuilder),
            like_condition_parts("a\\%_B", PostgresQueryBuilder),
        ] {
            assert!(sql.contains("LIKE"), "expected LIKE in: {sql}");
            assert_eq!(values, expected);
        }
    }

    #[test]
    fn lower_like_condition_renders_sqlite_sql_with_escape_clause() {
        let sql = Query::select()
            .column(Alias::new("name"))
            .from(Alias::new("items"))
            .and_where(lower_like_condition(NameColumn, "a\\%"))
            .to_string(SqliteQueryBuilder);
        assert_eq!(
            sql,
            r#"SELECT "name" FROM "items" WHERE LOWER("name") LIKE '%a\\\%%' ESCAPE '\'"#
        );
    }

    #[test]
    fn sqlite_match_query_wraps_multi_character_input_in_phrase_quotes() {
        assert_eq!(sqlite_match_query("report"), Some("\"report\"".into()));
        assert_eq!(
            sqlite_match_query("report\"2026"),
            Some("\"report\"\"2026\"".into())
        );
    }

    #[test]
    fn sqlite_match_query_falls_back_for_short_input() {
        assert_eq!(sqlite_match_query("r"), None);
        assert_eq!(sqlite_match_query("re"), None);
    }

    #[test]
    fn mysql_boolean_mode_query_uses_phrase_search_for_multi_char_input() {
        assert_eq!(
            mysql_boolean_mode_query("report"),
            Some("\"report\"".into())
        );
        assert_eq!(
            mysql_boolean_mode_query("report2026"),
            Some("\"report2026\"".into())
        );
    }

    #[test]
    fn mysql_boolean_mode_query_falls_back_for_invalid_input() {
        assert_eq!(mysql_boolean_mode_query("r"), None);
        assert_eq!(mysql_boolean_mode_query("re"), None);
        assert_eq!(mysql_boolean_mode_query("re-port"), None);
    }
}
