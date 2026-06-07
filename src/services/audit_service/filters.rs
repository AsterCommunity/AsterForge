use chrono::{DateTime, Utc};
use serde::Deserialize;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::{IntoParams, ToSchema};

use crate::types::AuditEntityType;

#[derive(Debug, Deserialize)]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    derive(IntoParams, ToSchema)
)]
pub struct AuditLogFilterQuery {
    pub user_id: Option<i64>,
    pub action: Option<String>,
    pub entity_type: Option<AuditEntityType>,
    pub entity_id: Option<i64>,
    pub after: Option<String>,
    pub before: Option<String>,
}

pub struct AuditLogFilters {
    pub user_id: Option<i64>,
    pub action: Option<String>,
    pub entity_type: Option<AuditEntityType>,
    pub entity_id: Option<i64>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

impl AuditLogFilters {
    pub fn from_query(query: &AuditLogFilterQuery) -> Self {
        Self {
            user_id: query.user_id,
            action: query.action.clone(),
            entity_type: query.entity_type,
            entity_id: query.entity_id,
            after: query
                .after
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|datetime| datetime.with_timezone(&Utc)),
            before: query
                .before
                .as_deref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|datetime| datetime.with_timezone(&Utc)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuditLogFilterQuery, AuditLogFilters};
    use crate::types::AuditEntityType;

    #[test]
    fn from_query_preserves_scalar_filters_and_parses_rfc3339_bounds() {
        let filters = AuditLogFilters::from_query(&AuditLogFilterQuery {
            user_id: Some(42),
            action: Some("config_update".to_string()),
            entity_type: Some(AuditEntityType::SystemConfig),
            entity_id: Some(7),
            after: Some("2026-06-06T01:02:03+08:00".to_string()),
            before: Some("2026-06-07T01:02:03Z".to_string()),
        });

        assert_eq!(filters.user_id, Some(42));
        assert_eq!(filters.action.as_deref(), Some("config_update"));
        assert_eq!(filters.entity_type, Some(AuditEntityType::SystemConfig));
        assert_eq!(filters.entity_id, Some(7));
        assert_eq!(
            filters.after.unwrap().to_rfc3339(),
            "2026-06-05T17:02:03+00:00"
        );
        assert_eq!(
            filters.before.unwrap().to_rfc3339(),
            "2026-06-07T01:02:03+00:00"
        );
    }

    #[test]
    fn from_query_ignores_invalid_timestamp_bounds() {
        let filters = AuditLogFilters::from_query(&AuditLogFilterQuery {
            user_id: None,
            action: None,
            entity_type: None,
            entity_id: None,
            after: Some("not-a-date".to_string()),
            before: Some("2026-99-99T00:00:00Z".to_string()),
        });

        assert_eq!(filters.user_id, None);
        assert_eq!(filters.action, None);
        assert_eq!(filters.entity_type, None);
        assert_eq!(filters.entity_id, None);
        assert_eq!(filters.after, None);
        assert_eq!(filters.before, None);
    }
}
