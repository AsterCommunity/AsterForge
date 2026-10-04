//! Shared API response and pagination helpers for Aster services.
//!
//! This crate contains small HTTP-facing types that are useful across service boundaries:
//! bounded limit query parsing, limit/offset pagination, cursor-page response shapes, cursor
//! validation helpers, overfetch trimming, and simple sort-order serialization. It deliberately
//! avoids depending on any concrete web framework or product entity so handlers can adapt it to
//! Axum, Actix, `OpenAPI` generation, or test-only fixtures.
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

mod cursor;
mod error;
mod pagination;
mod patch;
/// Typed, framework-neutral REST response envelopes.
pub mod response;
mod schema;
mod sort;

pub use cursor::{
    CreatedAtCursorQuery, CursorPage, CursorSlice, DateTimeIdCursor, DateTimeStringCursor,
    EnabledPriorityIdCursor, IdCursor, SortOrderNameIdCursor, StringIdCursor, UpdatedAtCursorQuery,
    parse_datetime_id_cursor, parse_datetime_string_cursor, parse_enabled_priority_id_cursor,
    parse_id_cursor, parse_sort_order_name_id_cursor, parse_string_id_cursor,
};
pub use error::{ApiError, Result};
pub use pagination::{
    DEFAULT_FILE_LIMIT, DEFAULT_FOLDER_LIMIT, DEFAULT_PAGE_LIMIT, LimitOffsetQuery, LimitQuery,
    MAX_PAGE_SIZE, OffsetPage, load_offset_page,
};
pub use patch::{NullablePatch, deserialize_nullable_patch_option};
#[doc(hidden)]
pub use schema::ApiSchema;
pub use sort::SortOrder;
