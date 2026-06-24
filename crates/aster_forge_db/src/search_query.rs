//! Shared search-query expression helpers.
//!
//! The helpers in this module build small SeaQuery expressions for common
//! search behavior used by Aster repositories: escaped `LIKE` patterns,
//! case-insensitive substring checks, SQLite FTS phrase queries, and MySQL
//! boolean-mode phrase queries. They do not depend on product entities and leave
//! database-specific query composition to the caller.

use sea_orm::ExprTrait;
use sea_orm::sea_query::{
    Alias, Expr, Func, IntoColumnRef, Query, SimpleExpr, extension::sqlite::SqliteExpr,
};

/// Escapes wildcard characters for SQL `LIKE` queries.
pub fn escape_like_query(query: &str) -> String {
    query.replace('%', "\\%").replace('_', "\\_")
}

/// Builds a case-insensitive `LIKE '%query%'` condition for a column.
pub fn lower_like_condition(column: impl IntoColumnRef + Copy, query: &str) -> SimpleExpr {
    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for ch in query.chars() {
        match ch {
            '%' => pattern.push_str("\\%"),
            '_' => pattern.push_str("\\_"),
            _ => pattern.extend(ch.to_lowercase()),
        }
    }
    pattern.push('%');
    Expr::expr(Func::lower(Expr::col(column))).like(pattern)
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
    use super::{escape_like_query, mysql_boolean_mode_query, sqlite_match_query};

    #[test]
    fn escape_like_query_escapes_wildcards() {
        assert_eq!(escape_like_query("100%_done"), "100\\%\\_done");
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
