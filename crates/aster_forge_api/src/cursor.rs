//! Cursor queries, response shapes, completeness checks and overfetch trimming.
use crate::{ApiError, ApiSchema, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::{IntoParams, ToSchema};
/// Cursor query for resources ordered by creation time and numeric id.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct CreatedAtCursorQuery {
    /// Cursor creation timestamp.
    pub after_created_at: Option<DateTime<Utc>>,
    /// Cursor numeric id used as a stable tie breaker.
    pub after_id: Option<i64>,
}

/// Cursor query for resources ordered by update time and numeric id.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct UpdatedAtCursorQuery {
    /// Cursor update timestamp.
    pub after_updated_at: Option<DateTime<Utc>>,
    /// Cursor numeric id used as a stable tie breaker.
    pub after_id: Option<i64>,
}

/// Serialized cursor page response.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct CursorPage<T: Serialize + ApiSchema, C: Serialize + ApiSchema> {
    /// Items in the current page.
    pub items: Vec<T>,
    /// Total number of items matching the query.
    pub total: u64,
    /// Effective page size.
    pub limit: u64,
    /// Cursor that can be sent back to fetch the next page.
    pub next_cursor: Option<C>,
}

impl<T: Serialize + ApiSchema, C: Serialize + ApiSchema> CursorPage<T, C> {
    /// Creates a new cursor page.
    pub fn new(items: Vec<T>, total: u64, limit: u64, next_cursor: Option<C>) -> Self {
        Self {
            items,
            total,
            limit,
            next_cursor,
        }
    }
}

/// Numeric id cursor for resources sorted by id.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct IdCursor {
    /// Cursor id.
    pub id: i64,
}

/// String value plus numeric id cursor for resources sorted by text and then id.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct StringIdCursor {
    /// Cursor string value.
    pub value: String,
    /// Cursor numeric id used as a stable tie breaker.
    pub id: i64,
}

/// Sort-order, name, and numeric id cursor for manually ordered named resources.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct SortOrderNameIdCursor {
    /// Cursor sort order value.
    pub sort_order: i32,
    /// Cursor display or storage name.
    pub name: String,
    /// Cursor numeric id used as a stable tie breaker.
    pub id: i64,
}

/// Enabled flag, priority, and numeric id cursor for prioritized toggle-like resources.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct EnabledPriorityIdCursor {
    /// Cursor enabled flag.
    pub enabled: bool,
    /// Cursor priority value.
    pub priority: i32,
    /// Cursor numeric id used as a stable tie breaker.
    pub id: i64,
}

/// Timestamp plus numeric id cursor for resources sorted by time and then id.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct DateTimeIdCursor {
    /// Cursor timestamp.
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub value: DateTime<Utc>,
    /// Cursor numeric id used as a stable tie breaker.
    pub id: i64,
}

/// Timestamp plus string id cursor for resources sorted by time and then string id.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct DateTimeStringCursor {
    /// Cursor timestamp.
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub value: DateTime<Utc>,
    /// Cursor string id used as a stable tie breaker.
    pub id: String,
}

/// Validates a timestamp plus numeric id cursor pair.
///
/// # Errors
///
/// Returns an error when only one cursor component is present or the id is not positive.
pub fn parse_datetime_id_cursor(
    value: Option<DateTime<Utc>>,
    id: Option<i64>,
    value_name: &str,
) -> Result<Option<(DateTime<Utc>, i64)>> {
    match (value, id) {
        (None, None) => Ok(None),
        (Some(value), Some(id)) if id > 0 => Ok(Some((value, id))),
        (Some(_), Some(_)) => Err(ApiError::new(format!(
            "{value_name} cursor id must be positive",
        ))),
        _ => Err(ApiError::new(format!(
            "{value_name} cursor requires both value and id",
        ))),
    }
}

/// Validates a timestamp plus string id cursor pair.
///
/// # Errors
///
/// Returns an error when only one cursor component is present or the string id is blank.
pub fn parse_datetime_string_cursor(
    value: Option<DateTime<Utc>>,
    id: Option<String>,
    value_name: &str,
) -> Result<Option<(DateTime<Utc>, String)>> {
    match (value, id) {
        (None, None) => Ok(None),
        (Some(value), Some(id)) if !id.trim().is_empty() => Ok(Some((value, id))),
        (Some(_), Some(_)) => Err(ApiError::new(format!(
            "{value_name} cursor id must not be empty",
        ))),
        _ => Err(ApiError::new(format!(
            "{value_name} cursor requires both value and id",
        ))),
    }
}

/// Validates an optional positive numeric id cursor.
///
/// # Errors
///
/// Returns an error when the supplied id is zero or negative.
pub fn parse_id_cursor(id: Option<i64>, value_name: &str) -> Result<Option<i64>> {
    match id {
        None => Ok(None),
        Some(id) if id > 0 => Ok(Some(id)),
        Some(_) => Err(ApiError::new(format!(
            "{value_name} cursor id must be positive",
        ))),
    }
}

/// Validates a string value plus numeric id cursor pair.
///
/// # Errors
///
/// Returns an error when the tuple is incomplete, the string value is blank, or the id is not
/// positive.
pub fn parse_string_id_cursor(
    value: Option<String>,
    id: Option<i64>,
    value_name: &str,
) -> Result<Option<(String, i64)>> {
    match (value, id) {
        (None, None) => Ok(None),
        (Some(value), Some(id)) if !value.trim().is_empty() && id > 0 => Ok(Some((value, id))),
        (Some(_), Some(id)) if id <= 0 => Err(ApiError::new(format!(
            "{value_name} cursor id must be positive",
        ))),
        (Some(_), Some(_)) => Err(ApiError::new(format!(
            "{value_name} cursor value must not be empty",
        ))),
        _ => Err(ApiError::new(format!(
            "{value_name} cursor requires both value and id",
        ))),
    }
}

/// Validates a sort-order, name, and numeric id cursor tuple.
///
/// # Errors
///
/// Returns an error when the tuple is incomplete, the name is blank, or the id is not positive.
pub fn parse_sort_order_name_id_cursor(
    sort_order: Option<i32>,
    name: Option<String>,
    id: Option<i64>,
    value_name: &str,
) -> Result<Option<(i32, String, i64)>> {
    match (sort_order, name, id) {
        (None, None, None) => Ok(None),
        (Some(sort_order), Some(name), Some(id)) if !name.trim().is_empty() && id > 0 => {
            Ok(Some((sort_order, name, id)))
        }
        (Some(_), Some(_), Some(id)) if id <= 0 => Err(ApiError::new(format!(
            "{value_name} cursor id must be positive",
        ))),
        (Some(_), Some(_), Some(_)) => Err(ApiError::new(format!(
            "{value_name} cursor name must not be empty",
        ))),
        _ => Err(ApiError::new(format!(
            "{value_name} cursor requires sort_order, name, and id",
        ))),
    }
}

/// Validates an enabled flag, priority, and numeric id cursor tuple.
///
/// # Errors
///
/// Returns an error when the tuple is incomplete or the id is not positive.
pub fn parse_enabled_priority_id_cursor(
    enabled: Option<bool>,
    priority: Option<i32>,
    id: Option<i64>,
    value_name: &str,
) -> Result<Option<(bool, i32, i64)>> {
    match (enabled, priority, id) {
        (None, None, None) => Ok(None),
        (Some(enabled), Some(priority), Some(id)) if id > 0 => Ok(Some((enabled, priority, id))),
        (Some(_), Some(_), Some(_)) => Err(ApiError::new(format!(
            "{value_name} cursor id must be positive",
        ))),
        _ => Err(ApiError::new(format!(
            "{value_name} cursor requires enabled, priority, and id",
        ))),
    }
}

/// Repository page slice returned after fetching one extra row to detect a next page.
#[derive(Debug, Clone)]
pub struct CursorSlice<T> {
    /// Items to expose to the caller after overfetch trimming.
    pub items: Vec<T>,
    /// Total number of items matching the query.
    pub total: u64,
    /// Whether the repository found at least one item beyond the requested limit.
    pub has_more: bool,
}

impl<T> CursorSlice<T> {
    /// Creates an empty slice with a known total count.
    #[must_use]
    pub fn empty(total: u64) -> Self {
        Self {
            items: Vec::new(),
            total,
            has_more: false,
        }
    }

    /// Builds a cursor slice from a repository result that fetched `limit + 1` rows.
    ///
    /// # Errors
    ///
    /// Returns an error when the item count or truncation limit cannot be represented by the
    /// required integer type on the current platform.
    pub fn from_overfetch(mut items: Vec<T>, total: u64, limit: u64) -> Result<Self> {
        let item_count = u64::try_from(items.len())
            .map_err(|_| ApiError::new("cursor slice item count is too large"))?;
        let has_more = item_count > limit;
        if has_more {
            let limit =
                usize::try_from(limit).map_err(|_| ApiError::new("cursor limit is too large"))?;
            items.truncate(limit);
        }
        Ok(Self {
            items,
            total,
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cursor_tuple_presence_combinations_and_integer_extremes_are_checked() {
        for mask in 0..8 {
            let first = (mask & 1 != 0).then_some(i32::MIN);
            let name = (mask & 2 != 0).then(|| "名字".to_string());
            let id = (mask & 4 != 0).then_some(i64::MAX);
            assert_eq!(
                parse_sort_order_name_id_cursor(first, name, id, "resource").is_ok(),
                mask == 0 || mask == 7
            );
            let enabled = (mask & 1 != 0).then_some(false);
            let priority = (mask & 2 != 0).then_some(i32::MAX);
            assert_eq!(
                parse_enabled_priority_id_cursor(enabled, priority, id, "resource").is_ok(),
                mask == 0 || mask == 7
            );
        }
        for id in [i64::MIN, -1, 0] {
            assert!(parse_id_cursor(Some(id), "resource").is_err());
            assert!(parse_string_id_cursor(Some("value".into()), Some(id), "resource").is_err());
            assert!(
                parse_sort_order_name_id_cursor(
                    Some(0),
                    Some("value".into()),
                    Some(id),
                    "resource"
                )
                .is_err()
            );
            assert!(
                parse_enabled_priority_id_cursor(Some(false), Some(0), Some(id), "resource")
                    .is_err()
            );
        }
        assert_eq!(
            parse_id_cursor(Some(i64::MAX), "resource").unwrap(),
            Some(i64::MAX)
        );
        for blank in ["", " ", "\t\n", "\u{2003}"] {
            assert!(parse_string_id_cursor(Some(blank.into()), Some(1), "resource").is_err());
            assert!(
                parse_datetime_string_cursor(
                    Some(DateTime::UNIX_EPOCH),
                    Some(blank.into()),
                    "resource"
                )
                .is_err()
            );
        }
        let value = " 名字 ".to_string();
        assert_eq!(
            parse_string_id_cursor(Some(value.clone()), Some(1), "resource").unwrap(),
            Some((value, 1))
        );
    }

    #[test]
    fn overfetch_handles_zero_limit_empty_items_and_large_limits_without_truncation() {
        for (items, limit, expected, has_more) in [
            (vec![], 0, vec![], false),
            (vec![1], 0, vec![], true),
            (vec![1, 2], 2, vec![1, 2], false),
            (vec![1, 2], 1, vec![1], true),
            (vec![1, 2], u64::MAX, vec![1, 2], false),
        ] {
            let slice = CursorSlice::from_overfetch(items, u64::MAX, limit).unwrap();
            assert_eq!(
                (slice.items, slice.total, slice.has_more),
                (expected, u64::MAX, has_more)
            );
        }
    }
    #[test]
    fn cursor_page_serializes_expected_shape() {
        let page = CursorPage::new(vec!["a", "b"], 10, 2, Some(IdCursor { id: 42 }));
        let value = serde_json::to_value(page).unwrap();

        assert_eq!(
            value,
            json!({
                "items": ["a", "b"],
                "total": 10,
                "limit": 2,
                "next_cursor": { "id": 42 }
            })
        );
    }

    #[test]
    fn parse_id_cursor_accepts_absent_or_positive_id() {
        assert_eq!(parse_id_cursor(None, "profile").unwrap(), None);
        assert_eq!(parse_id_cursor(Some(7), "profile").unwrap(), Some(7));

        let error = parse_id_cursor(Some(0), "profile").unwrap_err();
        assert_eq!(error.message(), "profile cursor id must be positive");
    }

    #[test]
    fn parse_datetime_id_cursor_requires_both_parts() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            parse_datetime_id_cursor(Some(timestamp), Some(9), "audit")
                .unwrap()
                .unwrap(),
            (timestamp, 9)
        );
        assert_eq!(parse_datetime_id_cursor(None, None, "audit").unwrap(), None);

        let error = parse_datetime_id_cursor(Some(timestamp), None, "audit").unwrap_err();
        assert_eq!(error.message(), "audit cursor requires both value and id");

        let error = parse_datetime_id_cursor(Some(timestamp), Some(-1), "audit").unwrap_err();
        assert_eq!(error.message(), "audit cursor id must be positive");
    }

    #[test]
    fn parse_datetime_string_cursor_rejects_empty_id() {
        let timestamp = DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(
            parse_datetime_string_cursor(Some(timestamp), Some("abc".to_string()), "session")
                .unwrap()
                .unwrap(),
            (timestamp, "abc".to_string())
        );

        let error = parse_datetime_string_cursor(Some(timestamp), Some(" ".to_string()), "session")
            .unwrap_err();
        assert_eq!(error.message(), "session cursor id must not be empty");
    }

    #[test]
    fn parse_string_id_cursor_rejects_incomplete_or_empty_values() {
        assert_eq!(
            parse_string_id_cursor(Some("oauth".to_string()), Some(3), "provider")
                .unwrap()
                .unwrap(),
            ("oauth".to_string(), 3)
        );

        let error = parse_string_id_cursor(Some(" ".to_string()), Some(3), "provider").unwrap_err();
        assert_eq!(error.message(), "provider cursor value must not be empty");

        let error =
            parse_string_id_cursor(Some("oauth".to_string()), Some(0), "provider").unwrap_err();
        assert_eq!(error.message(), "provider cursor id must be positive");

        let error =
            parse_string_id_cursor(Some("oauth".to_string()), None, "provider").unwrap_err();
        assert_eq!(
            error.message(),
            "provider cursor requires both value and id"
        );
    }

    #[test]
    fn parse_sort_order_name_id_cursor_validates_tuple() {
        assert_eq!(
            parse_sort_order_name_id_cursor(Some(10), Some("cape".to_string()), Some(2), "tag")
                .unwrap()
                .unwrap(),
            (10, "cape".to_string(), 2)
        );
        assert_eq!(
            parse_sort_order_name_id_cursor(None, None, None, "tag").unwrap(),
            None
        );

        let error =
            parse_sort_order_name_id_cursor(Some(10), Some(" ".to_string()), Some(2), "tag")
                .unwrap_err();
        assert_eq!(error.message(), "tag cursor name must not be empty");

        let error =
            parse_sort_order_name_id_cursor(Some(10), Some("cape".to_string()), None, "tag")
                .unwrap_err();
        assert_eq!(
            error.message(),
            "tag cursor requires sort_order, name, and id"
        );
    }

    #[test]
    fn parse_enabled_priority_id_cursor_validates_tuple() {
        assert_eq!(
            parse_enabled_priority_id_cursor(Some(true), Some(10), Some(2), "server")
                .unwrap()
                .unwrap(),
            (true, 10, 2)
        );
        assert_eq!(
            parse_enabled_priority_id_cursor(None, None, None, "server").unwrap(),
            None
        );

        let error =
            parse_enabled_priority_id_cursor(Some(true), Some(10), Some(0), "server").unwrap_err();
        assert_eq!(error.message(), "server cursor id must be positive");

        let error =
            parse_enabled_priority_id_cursor(Some(true), Some(10), None, "server").unwrap_err();
        assert_eq!(
            error.message(),
            "server cursor requires enabled, priority, and id"
        );
    }

    #[test]
    fn cursor_slice_trims_overfetch_and_reports_has_more() {
        let slice = CursorSlice::from_overfetch(vec![1, 2, 3], 10, 2).unwrap();
        assert_eq!(slice.items, vec![1, 2]);
        assert_eq!(slice.total, 10);
        assert!(slice.has_more);

        let slice = CursorSlice::from_overfetch(vec![1, 2], 2, 2).unwrap();
        assert_eq!(slice.items, vec![1, 2]);
        assert_eq!(slice.total, 2);
        assert!(!slice.has_more);

        let slice = CursorSlice::<u8>::empty(7);
        assert!(slice.items.is_empty());
        assert_eq!(slice.total, 7);
        assert!(!slice.has_more);
    }
}
