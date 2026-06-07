//! Audit log repository.

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Select,
};

use crate::api::pagination::{AdminAuditLogSortBy, SortOrder};
use crate::db::repository::sort::{order_by_column_with_id, order_by_id};
use crate::entities::audit_log::{self, Entity as AuditLog};
use crate::errors::{AsterError, Result};

pub struct AuditLogQuery<'a> {
    pub user_id: Option<i64>,
    pub action: Option<&'a str>,
    pub entity_type: Option<&'a str>,
    pub entity_id: Option<i64>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: u64,
    pub offset: u64,
    pub sort_by: AdminAuditLogSortBy,
    pub sort_order: SortOrder,
}

pub async fn create<C: ConnectionTrait>(
    db: &C,
    model: audit_log::ActiveModel,
) -> Result<audit_log::Model> {
    model.insert(db).await.map_err(AsterError::from)
}

pub async fn create_many<C: ConnectionTrait>(
    db: &C,
    models: Vec<audit_log::ActiveModel>,
) -> Result<()> {
    if models.is_empty() {
        return Ok(());
    }
    AuditLog::insert_many(models)
        .exec(db)
        .await
        .map_err(AsterError::from)?;
    Ok(())
}

pub async fn find_with_filters<C: ConnectionTrait>(
    db: &C,
    query: AuditLogQuery<'_>,
) -> Result<(Vec<audit_log::Model>, u64)> {
    let mut q = apply_admin_audit_log_sort(AuditLog::find(), query.sort_by, query.sort_order);

    if let Some(user_id) = query.user_id {
        q = q.filter(audit_log::Column::UserId.eq(user_id));
    }
    if let Some(action) = query.action {
        q = q.filter(audit_log::Column::Action.eq(action));
    }
    if let Some(entity_type) = query.entity_type {
        q = q.filter(audit_log::Column::EntityType.eq(entity_type));
    }
    if let Some(entity_id) = query.entity_id {
        q = q.filter(audit_log::Column::EntityId.eq(entity_id));
    }
    if let Some(after) = query.after {
        q = q.filter(audit_log::Column::CreatedAt.gte(after));
    }
    if let Some(before) = query.before {
        q = q.filter(audit_log::Column::CreatedAt.lte(before));
    }

    let total = q.clone().count(db).await.map_err(AsterError::from)?;
    let items = q
        .limit(query.limit)
        .offset(query.offset)
        .all(db)
        .await
        .map_err(AsterError::from)?;

    Ok((items, total))
}

pub async fn list_recent<C: ConnectionTrait>(
    db: &C,
    limit: u64,
    offset: u64,
) -> Result<(Vec<audit_log::Model>, u64)> {
    let query = AuditLog::find()
        .order_by_desc(audit_log::Column::CreatedAt)
        .order_by_desc(audit_log::Column::Id);
    let total = query.clone().count(db).await.map_err(AsterError::from)?;
    let items = query
        .limit(limit)
        .offset(offset)
        .all(db)
        .await
        .map_err(AsterError::from)?;
    Ok((items, total))
}

fn apply_admin_audit_log_sort(
    query: Select<AuditLog>,
    sort_by: AdminAuditLogSortBy,
    sort_order: SortOrder,
) -> Select<AuditLog> {
    match sort_by {
        AdminAuditLogSortBy::Id => order_by_id(query, audit_log::Column::Id, sort_order),
        AdminAuditLogSortBy::CreatedAt => order_by_column_with_id(
            query,
            audit_log::Column::CreatedAt,
            sort_order,
            audit_log::Column::Id,
        ),
        AdminAuditLogSortBy::UserId => order_by_column_with_id(
            query,
            audit_log::Column::UserId,
            sort_order,
            audit_log::Column::Id,
        ),
        AdminAuditLogSortBy::Action => order_by_column_with_id(
            query,
            audit_log::Column::Action,
            sort_order,
            audit_log::Column::Id,
        ),
        AdminAuditLogSortBy::EntityType => order_by_column_with_id(
            query,
            audit_log::Column::EntityType,
            sort_order,
            audit_log::Column::Id,
        ),
        AdminAuditLogSortBy::EntityName => order_by_column_with_id(
            query,
            audit_log::Column::EntityName,
            sort_order,
            audit_log::Column::Id,
        ),
        AdminAuditLogSortBy::IpAddress => order_by_column_with_id(
            query,
            audit_log::Column::IpAddress,
            sort_order,
            audit_log::Column::Id,
        ),
    }
}

pub async fn delete_before<C: ConnectionTrait>(db: &C, before: DateTime<Utc>) -> Result<u64> {
    let result = AuditLog::delete_many()
        .filter(audit_log::Column::CreatedAt.lt(before))
        .exec(db)
        .await
        .map_err(AsterError::from)?;
    Ok(result.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::{
        AuditLogQuery, create, create_many, delete_before, find_with_filters, list_recent,
    };
    use crate::api::pagination::{AdminAuditLogSortBy, SortOrder};
    use crate::config::DatabaseConfig;
    use crate::entities::audit_log;
    use crate::types::{AuditAction, AuditEntityType};
    use chrono::{Duration, Utc};
    use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};

    async fn build_test_db() -> sea_orm::DatabaseConnection {
        let db = crate::db::connect_with_metrics(
            &DatabaseConfig {
                url: "sqlite::memory:".to_string(),
                pool_size: 1,
                retry_count: 0,
            },
            crate::metrics_core::NoopMetrics::arc(),
        )
        .await
        .expect("audit log repo test DB should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("audit log repo test migrations should succeed");
        db
    }

    fn audit_model(
        user_id: i64,
        action: AuditAction,
        entity_type: AuditEntityType,
        entity_id: Option<i64>,
        entity_name: &str,
        ip_address: &str,
        created_at: chrono::DateTime<Utc>,
    ) -> audit_log::ActiveModel {
        audit_log::ActiveModel {
            id: Default::default(),
            user_id: Set(user_id),
            action: Set(action),
            entity_type: Set(entity_type.as_str().to_string()),
            entity_id: Set(entity_id),
            entity_name: Set(Some(entity_name.to_string())),
            details: Set(Some(
                serde_json::json!({
                    "entity_name": entity_name,
                    "action": action.as_str(),
                })
                .to_string(),
            )),
            ip_address: Set(Some(ip_address.to_string())),
            user_agent: Set(Some("audit-repo-test".to_string())),
            created_at: Set(created_at),
        }
    }

    async fn all_audit_count(db: &sea_orm::DatabaseConnection) -> u64 {
        audit_log::Entity::find()
            .count(db)
            .await
            .expect("audit log count should succeed")
    }

    fn unfiltered_query(
        limit: u64,
        offset: u64,
        sort_by: AdminAuditLogSortBy,
        sort_order: SortOrder,
    ) -> AuditLogQuery<'static> {
        AuditLogQuery {
            user_id: None,
            action: None,
            entity_type: None,
            entity_id: None,
            after: None,
            before: None,
            limit,
            offset,
            sort_by,
            sort_order,
        }
    }

    #[tokio::test]
    async fn create_and_empty_create_many_follow_repository_contract() {
        let db = build_test_db().await;
        create_many(&db, Vec::new()).await.unwrap();
        assert_eq!(all_audit_count(&db).await, 0);

        let created_at = Utc::now();
        let entry = create(
            &db,
            audit_model(
                42,
                AuditAction::UserLogin,
                AuditEntityType::AuthSession,
                Some(7),
                "admin",
                "127.0.0.1",
                created_at,
            ),
        )
        .await
        .unwrap();

        assert!(entry.id > 0);
        assert_eq!(entry.user_id, 42);
        assert_eq!(entry.action, AuditAction::UserLogin);
        assert_eq!(entry.entity_type, "auth_session");
        assert_eq!(entry.entity_id, Some(7));
        assert_eq!(entry.entity_name.as_deref(), Some("admin"));
        assert_eq!(entry.ip_address.as_deref(), Some("127.0.0.1"));
        assert_eq!(all_audit_count(&db).await, 1);

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn find_with_filters_applies_all_supported_predicates() {
        let db = build_test_db().await;
        let base = Utc::now();
        let ignored_by_action = create(
            &db,
            audit_model(
                10,
                AuditAction::UserLogin,
                AuditEntityType::AuthSession,
                Some(1),
                "login",
                "192.0.2.10",
                base - Duration::seconds(30),
            ),
        )
        .await
        .unwrap();
        let expected = create(
            &db,
            audit_model(
                10,
                AuditAction::ConfigUpdate,
                AuditEntityType::SystemConfig,
                Some(2),
                "branding_title",
                "192.0.2.11",
                base - Duration::seconds(20),
            ),
        )
        .await
        .unwrap();
        let ignored_by_user = create(
            &db,
            audit_model(
                20,
                AuditAction::ConfigUpdate,
                AuditEntityType::SystemConfig,
                Some(2),
                "branding_title",
                "192.0.2.12",
                base - Duration::seconds(20),
            ),
        )
        .await
        .unwrap();
        let ignored_by_time = create(
            &db,
            audit_model(
                10,
                AuditAction::ConfigUpdate,
                AuditEntityType::SystemConfig,
                Some(2),
                "late",
                "192.0.2.13",
                base - Duration::seconds(5),
            ),
        )
        .await
        .unwrap();

        let (items, total) = find_with_filters(
            &db,
            AuditLogQuery {
                user_id: Some(10),
                action: Some(AuditAction::ConfigUpdate.as_str()),
                entity_type: Some(AuditEntityType::SystemConfig.as_str()),
                entity_id: Some(2),
                after: Some(base - Duration::seconds(25)),
                before: Some(base - Duration::seconds(15)),
                limit: 10,
                offset: 0,
                sort_by: AdminAuditLogSortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await
        .unwrap();

        assert_eq!(total, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, expected.id);
        assert_ne!(items[0].id, ignored_by_action.id);
        assert_ne!(items[0].id, ignored_by_user.id);
        assert_ne!(items[0].id, ignored_by_time.id);

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn find_with_filters_supports_all_admin_sort_columns() {
        let db = build_test_db().await;
        let base = Utc::now();
        let first = create(
            &db,
            audit_model(
                20,
                AuditAction::UserLogout,
                AuditEntityType::User,
                Some(20),
                "gamma",
                "192.0.2.3",
                base + Duration::seconds(30),
            ),
        )
        .await
        .unwrap();
        let second = create(
            &db,
            audit_model(
                10,
                AuditAction::ConfigUpdate,
                AuditEntityType::SystemConfig,
                Some(10),
                "alpha",
                "192.0.2.1",
                base + Duration::seconds(10),
            ),
        )
        .await
        .unwrap();
        let third = create(
            &db,
            audit_model(
                30,
                AuditAction::AdminCleanupTasks,
                AuditEntityType::Task,
                Some(30),
                "beta",
                "192.0.2.2",
                base + Duration::seconds(20),
            ),
        )
        .await
        .unwrap();

        for (sort_by, expected_ids) in [
            (AdminAuditLogSortBy::Id, vec![first.id, second.id, third.id]),
            (
                AdminAuditLogSortBy::CreatedAt,
                vec![second.id, third.id, first.id],
            ),
            (
                AdminAuditLogSortBy::UserId,
                vec![second.id, first.id, third.id],
            ),
            (
                AdminAuditLogSortBy::Action,
                vec![third.id, second.id, first.id],
            ),
            (
                AdminAuditLogSortBy::EntityType,
                vec![second.id, third.id, first.id],
            ),
            (
                AdminAuditLogSortBy::EntityName,
                vec![second.id, third.id, first.id],
            ),
            (
                AdminAuditLogSortBy::IpAddress,
                vec![second.id, third.id, first.id],
            ),
        ] {
            let (items, total) =
                find_with_filters(&db, unfiltered_query(10, 0, sort_by, SortOrder::Asc))
                    .await
                    .unwrap();
            assert_eq!(total, 3);
            assert_eq!(
                items.into_iter().map(|item| item.id).collect::<Vec<_>>(),
                expected_ids,
                "unexpected audit sort order for {sort_by:?}"
            );
        }

        let (desc_items, _) = find_with_filters(
            &db,
            unfiltered_query(10, 0, AdminAuditLogSortBy::CreatedAt, SortOrder::Desc),
        )
        .await
        .unwrap();
        assert_eq!(
            desc_items
                .into_iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![first.id, third.id, second.id]
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn list_recent_pages_by_created_at_and_delete_before_keeps_cutoff() {
        let db = build_test_db().await;
        let cutoff = Utc::now();
        let old = create(
            &db,
            audit_model(
                1,
                AuditAction::ServerStart,
                AuditEntityType::System,
                None,
                "old",
                "192.0.2.1",
                cutoff - Duration::seconds(1),
            ),
        )
        .await
        .unwrap();
        let at_cutoff = create(
            &db,
            audit_model(
                1,
                AuditAction::ServerShutdown,
                AuditEntityType::System,
                None,
                "cutoff",
                "192.0.2.2",
                cutoff,
            ),
        )
        .await
        .unwrap();
        let recent = create(
            &db,
            audit_model(
                1,
                AuditAction::ConfigDelete,
                AuditEntityType::SystemConfig,
                Some(3),
                "recent",
                "192.0.2.3",
                cutoff + Duration::seconds(1),
            ),
        )
        .await
        .unwrap();

        let (items, total) = list_recent(&db, 2, 0).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(
            items.into_iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![recent.id, at_cutoff.id]
        );

        assert_eq!(delete_before(&db, cutoff).await.unwrap(), 1);
        assert!(
            audit_log::Entity::find()
                .filter(audit_log::Column::Id.eq(old.id))
                .one(&db)
                .await
                .unwrap()
                .is_none()
        );
        let (remaining, total) = list_recent(&db, 10, 0).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(
            remaining
                .into_iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![recent.id, at_cutoff.id]
        );

        db.close().await.unwrap();
    }
}
