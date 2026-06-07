use std::collections::{HashMap, HashSet};

use chrono::Utc;

use super::filters::AuditLogFilters;
use super::manager::flush_global_audit_log_manager;
use super::models::{AuditLogEntry, AuditUserSummary};
use super::presentation::build_audit_presentation;
use crate::api::pagination::{AdminAuditLogSortBy, OffsetPage, SortOrder, load_offset_page};
use crate::db::repository::{audit_log_repo, user_repo};
use crate::entities::audit_log;
use crate::runtime::SharedRuntimeState;
use crate::types::AuditEntityType;

async fn build_audit_entries<S: SharedRuntimeState>(
    state: &S,
    entries: Vec<audit_log::Model>,
) -> crate::errors::Result<Vec<AuditLogEntry>> {
    let user_ids = entries
        .iter()
        .map(|entry| entry.user_id)
        .filter(|user_id| *user_id > 0)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let users = user_repo::find_by_ids(state.reader_db(), &user_ids)
        .await?
        .into_iter()
        .map(|user| (user.id, AuditUserSummary::from(user)))
        .collect::<HashMap<_, _>>();

    let mut items = Vec::with_capacity(entries.len());
    for model in entries {
        let Some(entity_type) = AuditEntityType::from_str_name(&model.entity_type) else {
            tracing::warn!(
                audit_log_id = model.id,
                entity_type = %model.entity_type,
                "skipping audit log with unsupported entity_type"
            );
            continue;
        };

        let presentation = build_audit_presentation(
            model.action,
            entity_type,
            model.entity_id,
            model.entity_name.as_deref(),
            model.details.as_deref(),
        );

        items.push(AuditLogEntry {
            id: model.id,
            user_id: model.user_id,
            user: users.get(&model.user_id).cloned(),
            action: model.action,
            entity_type,
            entity_id: model.entity_id,
            entity_name: model.entity_name,
            details: model.details,
            presentation,
            ip_address: model.ip_address,
            user_agent: model.user_agent,
            created_at: model.created_at,
        });
    }

    Ok(items)
}

pub async fn query<S: SharedRuntimeState>(
    state: &S,
    filters: AuditLogFilters,
    limit: u64,
    offset: u64,
    sort_by: AdminAuditLogSortBy,
    sort_order: SortOrder,
) -> crate::errors::Result<OffsetPage<AuditLogEntry>> {
    flush_global_audit_log_manager().await;
    let page = load_offset_page(limit, offset, 200, |limit, offset| async move {
        audit_log_repo::find_with_filters(
            state.reader_db(),
            audit_log_repo::AuditLogQuery {
                user_id: filters.user_id,
                action: filters.action.as_deref(),
                entity_type: filters.entity_type.map(|entity_type| entity_type.as_str()),
                entity_id: filters.entity_id,
                after: filters.after,
                before: filters.before,
                limit,
                offset,
                sort_by,
                sort_order,
            },
        )
        .await
    })
    .await?;
    let items = build_audit_entries(state, page.items).await?;
    Ok(OffsetPage::new(items, page.total, page.limit, page.offset))
}

pub async fn cleanup_expired<S: SharedRuntimeState>(state: &S) -> crate::errors::Result<u64> {
    let retention_days = state
        .runtime_config()
        .get_i64("audit_log_retention_days")
        .filter(|days| *days > 0)
        .unwrap_or(90);
    let cutoff = Utc::now() - chrono::Duration::days(retention_days);
    audit_log_repo::delete_before(state.writer_db(), cutoff).await
}
