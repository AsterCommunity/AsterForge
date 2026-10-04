//! Transport-neutral sort direction.
use serde::{Deserialize, Serialize};
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
    fn sort_order_serializes_snake_case() {
        assert_eq!(serde_json::to_value(SortOrder::Asc).unwrap(), json!("asc"));
        assert_eq!(
            serde_json::to_value(SortOrder::Desc).unwrap(),
            json!("desc")
        );
    }
}
