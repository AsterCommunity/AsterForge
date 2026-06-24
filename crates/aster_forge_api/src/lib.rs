//! Shared API response and pagination helpers for Aster services.
//!
//! This crate contains small HTTP-facing types that are useful across service boundaries:
//! bounded limit query parsing, limit/offset pagination, cursor-page response shapes, cursor
//! validation helpers, overfetch trimming, and simple sort-order serialization. It deliberately
//! avoids depending on any concrete web framework or product entity so handlers can adapt it to
//! Axum, Actix, OpenAPI generation, or test-only fixtures.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::future::Future;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::{IntoParams, ToSchema};

/// Default page size for folder-list style endpoints.
pub const DEFAULT_FOLDER_LIMIT: u64 = 200;
/// Default page size for file-list style endpoints.
pub const DEFAULT_FILE_LIMIT: u64 = 100;
/// Default page size for cursor-based endpoints.
pub const DEFAULT_PAGE_LIMIT: u64 = 100;
/// Maximum accepted page size for offset and cursor pagination.
pub const MAX_PAGE_SIZE: u64 = 1000;

/// Result type returned by API helper functions.
pub type Result<T> = std::result::Result<T, ApiError>;

/// Error type for generic API helper failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    message: String,
}

impl ApiError {
    /// Creates an API helper error with a message.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Returns the stored error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Query parameters for limit/offset pagination.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct LimitOffsetQuery {
    /// Requested page size.
    pub limit: Option<u64>,
    /// Requested offset from the beginning of the result set.
    pub offset: Option<u64>,
}

impl LimitOffsetQuery {
    /// Returns the requested limit clamped to `[1, max]`, or `default` when absent.
    pub fn limit_or(&self, default: u64, max: u64) -> u64 {
        self.limit.map(|v| v.clamp(1, max)).unwrap_or(default)
    }

    /// Returns the requested offset, or zero when absent.
    pub fn offset(&self) -> u64 {
        self.offset.unwrap_or(0)
    }
}

/// Query parameters for limit-only cursor pagination.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct LimitQuery {
    /// Requested page size.
    pub limit: Option<u64>,
}

impl LimitQuery {
    /// Returns the requested limit clamped to `[1, max]`, or `default` when absent.
    pub fn limit_or(&self, default: u64, max: u64) -> u64 {
        self.limit
            .map(|value| value.clamp(1, max))
            .unwrap_or(default)
    }

    /// Returns the requested limit clamped against the crate's default cursor limits.
    pub fn limit(&self) -> u64 {
        self.limit_or(DEFAULT_PAGE_LIMIT, MAX_PAGE_SIZE)
    }
}

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

/// Three-state nullable field used by PATCH-style request DTOs.
///
/// `Absent` means the request omitted the field and the existing value should be preserved.
/// `Null` means the request explicitly supplied `null` and the existing value should be cleared.
/// `Value` means the request supplied a concrete replacement value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NullablePatch<T> {
    /// Field was omitted from the request.
    #[default]
    Absent,
    /// Field was present with JSON `null`.
    Null,
    /// Field was present with a concrete value.
    Value(T),
}

impl<T> NullablePatch<T> {
    /// Returns whether the request supplied this field as either `null` or a concrete value.
    pub fn is_present(&self) -> bool {
        !matches!(self, Self::Absent)
    }
}

/// Deserializes an optional PATCH field while preserving explicit `null`.
///
/// Use this with `#[serde(default, deserialize_with = "...")]` on `Option<NullablePatch<T>>`
/// fields when the surrounding DTO needs to distinguish omitted fields from explicit nulls.
pub fn deserialize_nullable_patch_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<NullablePatch<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(|value| Some(NullablePatch::from(value)))
}

impl<T> From<Option<T>> for NullablePatch<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        }
    }
}

impl<'de, T> Deserialize<'de> for NullablePatch<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

#[cfg(all(debug_assertions, feature = "openapi"))]
#[doc(hidden)]
pub trait ApiSchema: ToSchema {}

#[cfg(all(debug_assertions, feature = "openapi"))]
impl<T: ToSchema> ApiSchema for T {}

#[cfg(not(all(debug_assertions, feature = "openapi")))]
#[doc(hidden)]
pub trait ApiSchema {}

#[cfg(not(all(debug_assertions, feature = "openapi")))]
impl<T> ApiSchema for T {}

/// Serialized offset page response.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
pub struct OffsetPage<T: Serialize + ApiSchema> {
    /// Items in the current page.
    pub items: Vec<T>,
    /// Total number of items matching the query.
    pub total: u64,
    /// Effective page size.
    pub limit: u64,
    /// Offset used for this page.
    pub offset: u64,
}

impl<T: Serialize + ApiSchema> OffsetPage<T> {
    /// Creates a new offset page.
    pub fn new(items: Vec<T>, total: u64, limit: u64, offset: u64) -> Self {
        Self {
            items,
            total,
            limit,
            offset,
        }
    }
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

/// Loads an offset page by clamping `limit`, invoking `fetch`, and wrapping the result.
pub async fn load_offset_page<T, F, Fut>(
    limit: u64,
    offset: u64,
    max_limit: u64,
    fetch: F,
) -> Result<OffsetPage<T>>
where
    T: Serialize + ApiSchema,
    F: FnOnce(u64, u64) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, u64)>>,
{
    let limit = limit.clamp(1, max_limit);
    let (items, total) = fetch(limit, offset).await?;
    Ok(OffsetPage::new(items, total, limit, offset))
}

/// Validates a timestamp plus numeric id cursor pair.
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
    pub fn empty(total: u64) -> Self {
        Self {
            items: Vec::new(),
            total,
            has_more: false,
        }
    }

    /// Builds a cursor slice from a repository result that fetched `limit + 1` rows.
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

/// Sort direction used by API query parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SortOrder {
    /// Ascending order.
    #[default]
    Asc,
    /// Descending order.
    Desc,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn limit_offset_query_applies_defaults_and_bounds() {
        let query = LimitOffsetQuery {
            limit: None,
            offset: None,
        };
        assert_eq!(query.limit_or(50, 100), 50);
        assert_eq!(query.offset(), 0);

        let query = LimitOffsetQuery {
            limit: Some(0),
            offset: Some(25),
        };
        assert_eq!(query.limit_or(50, 100), 1);
        assert_eq!(query.offset(), 25);

        let query = LimitOffsetQuery {
            limit: Some(500),
            offset: Some(10),
        };
        assert_eq!(query.limit_or(50, 100), 100);
    }

    #[test]
    fn limit_query_applies_defaults_and_bounds() {
        let query = LimitQuery { limit: None };
        assert_eq!(query.limit_or(50, 100), 50);
        assert_eq!(query.limit(), DEFAULT_PAGE_LIMIT);

        let query = LimitQuery { limit: Some(0) };
        assert_eq!(query.limit_or(50, 100), 1);

        let query = LimitQuery { limit: Some(500) };
        assert_eq!(query.limit_or(50, 100), 100);
    }

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    struct PatchDto {
        #[serde(default)]
        title: NullablePatch<String>,
        #[serde(default, deserialize_with = "deserialize_nullable_patch_option")]
        description: Option<NullablePatch<String>>,
    }

    #[test]
    fn nullable_patch_deserializes_absent_null_and_value_fields() {
        let dto: PatchDto = serde_json::from_value(json!({})).unwrap();
        assert_eq!(dto.title, NullablePatch::Absent);
        assert_eq!(dto.description, None);
        assert!(!dto.title.is_present());

        let dto: PatchDto =
            serde_json::from_value(json!({ "title": null, "description": null })).unwrap();
        assert_eq!(dto.title, NullablePatch::Null);
        assert_eq!(dto.description, Some(NullablePatch::Null));
        assert!(dto.title.is_present());

        let dto: PatchDto = serde_json::from_value(json!({
            "title": "new title",
            "description": "new description"
        }))
        .unwrap();
        assert_eq!(dto.title, NullablePatch::Value("new title".to_string()));
        assert_eq!(
            dto.description,
            Some(NullablePatch::Value("new description".to_string()))
        );
    }

    #[test]
    fn offset_page_serializes_expected_shape() {
        let page = OffsetPage::new(vec!["a", "b"], 10, 2, 4);
        let value = serde_json::to_value(page).unwrap();

        assert_eq!(
            value,
            json!({
                "items": ["a", "b"],
                "total": 10,
                "limit": 2,
                "offset": 4
            })
        );
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

    #[tokio::test]
    async fn load_offset_page_clamps_limit_and_forwards_offset() {
        let page = load_offset_page(500, 30, 100, |limit, offset| async move {
            assert_eq!(limit, 100);
            assert_eq!(offset, 30);
            Ok((vec![1, 2, 3], 9))
        })
        .await
        .unwrap();

        assert_eq!(page.items, vec![1, 2, 3]);
        assert_eq!(page.total, 9);
        assert_eq!(page.limit, 100);
        assert_eq!(page.offset, 30);
    }

    #[tokio::test]
    async fn load_offset_page_propagates_fetch_error() {
        let error = load_offset_page::<u8, _, _>(10, 0, 100, |_limit, _offset| async {
            Err(ApiError::new("fetch failed"))
        })
        .await
        .unwrap_err();

        assert_eq!(error.message(), "fetch failed");
    }

    #[test]
    fn sort_order_serializes_snake_case() {
        assert_eq!(serde_json::to_value(SortOrder::Asc).unwrap(), json!("asc"));
        assert_eq!(
            serde_json::to_value(SortOrder::Desc).unwrap(),
            json!("desc")
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
