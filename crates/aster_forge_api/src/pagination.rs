//! Bounded pagination queries, offset responses and page-loading mechanics.
use crate::{ApiSchema, Result};
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
    #[must_use]
    pub fn limit_or(&self, default: u64, max: u64) -> u64 {
        self.limit.map_or(default, |v| v.clamp(1, max))
    }

    /// Returns the requested offset, or zero when absent.
    #[must_use]
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
    #[must_use]
    pub fn limit_or(&self, default: u64, max: u64) -> u64 {
        self.limit.map_or(default, |value| value.clamp(1, max))
    }

    /// Returns the requested limit clamped against the crate's default cursor limits.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit_or(DEFAULT_PAGE_LIMIT, MAX_PAGE_SIZE)
    }
}

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
    #[must_use]
    pub fn new(items: Vec<T>, total: u64, limit: u64, offset: u64) -> Self {
        Self {
            items,
            total,
            limit,
            offset,
        }
    }
}

/// Loads an offset page by clamping `limit`, invoking `fetch`, and wrapping the result.
///
/// # Errors
///
/// Returns the error produced by `fetch`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApiError;
    use serde_json::json;

    #[test]
    fn queries_preserve_large_offsets_and_clamp_limits_without_integer_loss() {
        let query = LimitOffsetQuery {
            limit: Some(u64::MAX),
            offset: Some(u64::MAX),
        };
        assert_eq!(query.limit_or(20, MAX_PAGE_SIZE), MAX_PAGE_SIZE);
        assert_eq!(query.offset(), u64::MAX);
        assert_eq!(
            LimitQuery {
                limit: Some(u64::MAX)
            }
            .limit(),
            MAX_PAGE_SIZE
        );
        assert_eq!(LimitQuery { limit: Some(0) }.limit_or(20, 1), 1);
        let page = OffsetPage::new(Vec::<u8>::new(), u64::MAX, MAX_PAGE_SIZE, u64::MAX);
        assert_eq!(
            serde_json::to_value(page).unwrap(),
            json!({"items":[],"total":u64::MAX,"limit":MAX_PAGE_SIZE,"offset":u64::MAX})
        );
    }
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
}
