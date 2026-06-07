//! External authentication repositories.

use crate::entities::{
    external_auth_identity, external_auth_login_flow,
    external_auth_provider::{self, Entity as ExternalAuthProvider},
};
use crate::errors::{AsterError, MapAsterErr, Result};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Set,
};

pub async fn list_enabled_providers<C: ConnectionTrait>(
    db: &C,
) -> Result<Vec<external_auth_provider::Model>> {
    ExternalAuthProvider::find()
        .filter(external_auth_provider::Column::Enabled.eq(true))
        .order_by_asc(external_auth_provider::Column::DisplayName)
        .all(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn list_enabled_providers_by_kind<C: ConnectionTrait>(
    db: &C,
    kind: crate::types::ExternalAuthKind,
) -> Result<Vec<external_auth_provider::Model>> {
    ExternalAuthProvider::find()
        .filter(external_auth_provider::Column::Enabled.eq(true))
        .filter(external_auth_provider::Column::Kind.eq(kind))
        .order_by_asc(external_auth_provider::Column::DisplayName)
        .all(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn list_providers_paginated<C: ConnectionTrait>(
    db: &C,
    limit: u64,
    offset: u64,
) -> Result<(Vec<external_auth_provider::Model>, u64)> {
    let query = ExternalAuthProvider::find()
        .order_by_asc(external_auth_provider::Column::DisplayName)
        .order_by_asc(external_auth_provider::Column::Id);
    let total = query
        .clone()
        .count(db)
        .await
        .map_aster_err(AsterError::database_operation)?;
    let items = query
        .limit(limit)
        .offset(offset)
        .all(db)
        .await
        .map_aster_err(AsterError::database_operation)?;
    Ok((items, total))
}

pub async fn find_provider_by_id<C: ConnectionTrait>(
    db: &C,
    id: i64,
) -> Result<external_auth_provider::Model> {
    ExternalAuthProvider::find_by_id(id)
        .one(db)
        .await
        .map_aster_err(AsterError::database_operation)?
        .ok_or_else(|| AsterError::record_not_found(format!("external auth provider #{id}")))
}

pub async fn find_provider_by_slug<C: ConnectionTrait>(
    db: &C,
    slug: &str,
) -> Result<Option<external_auth_provider::Model>> {
    ExternalAuthProvider::find()
        .filter(external_auth_provider::Column::Slug.eq(slug))
        .one(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn find_enabled_provider_by_slug<C: ConnectionTrait>(
    db: &C,
    slug: &str,
) -> Result<external_auth_provider::Model> {
    ExternalAuthProvider::find()
        .filter(external_auth_provider::Column::Slug.eq(slug))
        .filter(external_auth_provider::Column::Enabled.eq(true))
        .one(db)
        .await
        .map_aster_err(AsterError::database_operation)?
        .ok_or_else(|| AsterError::record_not_found(format!("external auth provider {slug}")))
}

pub async fn insert_provider<C: ConnectionTrait>(
    db: &C,
    active: external_auth_provider::ActiveModel,
) -> Result<external_auth_provider::Model> {
    active
        .insert(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn delete_provider<C: ConnectionTrait>(db: &C, id: i64) -> Result<()> {
    let result = ExternalAuthProvider::delete_by_id(id)
        .exec(db)
        .await
        .map_aster_err(AsterError::database_operation)?;
    if result.rows_affected == 0 {
        return Err(AsterError::record_not_found(format!(
            "external auth provider #{id}"
        )));
    }
    Ok(())
}

pub async fn create_login_flow<C: ConnectionTrait>(
    db: &C,
    provider_id: i64,
    state: &str,
    redirect_uri: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<external_auth_login_flow::Model> {
    external_auth_login_flow::ActiveModel {
        provider_id: Set(provider_id),
        state: Set(state.to_string()),
        redirect_uri: Set(redirect_uri.to_string()),
        expires_at: Set(expires_at),
        created_at: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_aster_err(AsterError::database_operation)
}

pub async fn consume_login_flow<C: ConnectionTrait>(
    db: &C,
    state: &str,
) -> Result<external_auth_login_flow::Model> {
    let flow = external_auth_login_flow::Entity::find()
        .filter(external_auth_login_flow::Column::State.eq(state))
        .one(db)
        .await
        .map_aster_err(AsterError::database_operation)?
        .ok_or_else(|| AsterError::external_auth_error("invalid external auth state"))?;

    if flow.consumed_at.is_some() || flow.expires_at <= chrono::Utc::now() {
        return Err(AsterError::external_auth_error(
            "external auth state expired or consumed",
        ));
    }

    let mut active: external_auth_login_flow::ActiveModel = flow.into();
    active.consumed_at = Set(Some(chrono::Utc::now()));
    active
        .update(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn cleanup_expired_login_flows<C: ConnectionTrait>(
    db: &C,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u64> {
    let result = external_auth_login_flow::Entity::delete_many()
        .filter(external_auth_login_flow::Column::ExpiresAt.lt(now))
        .exec(db)
        .await
        .map_aster_err(AsterError::database_operation)?;
    Ok(result.rows_affected)
}

#[allow(dead_code)]
pub async fn link_identity<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
    provider_id: i64,
    subject: &str,
    email: Option<String>,
    display_name: Option<String>,
) -> Result<external_auth_identity::Model> {
    external_auth_identity::ActiveModel {
        user_id: Set(user_id),
        provider_id: Set(provider_id),
        subject: Set(subject.to_string()),
        email: Set(email),
        display_name: Set(display_name),
        linked_at: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_aster_err(AsterError::database_operation)
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_expired_login_flows, consume_login_flow, create_login_flow, delete_provider,
        find_enabled_provider_by_slug, find_provider_by_id, find_provider_by_slug, insert_provider,
        link_identity, list_enabled_providers, list_enabled_providers_by_kind,
        list_providers_paginated,
    };
    use crate::config::DatabaseConfig;
    use crate::db::repository::user_repo;
    use crate::entities::external_auth_provider;
    use crate::types::{ExternalAuthKind, UserRole};
    use chrono::{Duration, Utc};
    use sea_orm::Set;

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
        .expect("external auth repo test DB should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("external auth repo test migrations should succeed");
        db
    }

    fn provider_active_model(
        slug: &str,
        display_name: &str,
        kind: ExternalAuthKind,
        enabled: bool,
    ) -> external_auth_provider::ActiveModel {
        let now = Utc::now();
        external_auth_provider::ActiveModel {
            slug: Set(slug.to_string()),
            display_name: Set(display_name.to_string()),
            kind: Set(kind),
            enabled: Set(enabled),
            issuer_url: Set(Some(format!("https://{slug}.example.com"))),
            authorize_url: Set(Some(format!("https://{slug}.example.com/authorize"))),
            token_url: Set(Some(format!("https://{slug}.example.com/token"))),
            userinfo_url: Set(Some(format!("https://{slug}.example.com/userinfo"))),
            client_id: Set(format!("{slug}-client")),
            client_secret: Set(format!("{slug}-secret")),
            scopes: Set("openid profile email".to_string()),
            created_at: Set(now),
            updated_at: Set(now),
            ..Default::default()
        }
    }

    async fn insert_test_provider(
        db: &sea_orm::DatabaseConnection,
        slug: &str,
        display_name: &str,
        kind: ExternalAuthKind,
        enabled: bool,
    ) -> external_auth_provider::Model {
        insert_provider(db, provider_active_model(slug, display_name, kind, enabled))
            .await
            .expect("external auth provider should insert")
    }

    async fn insert_user(db: &sea_orm::DatabaseConnection) -> i64 {
        user_repo::create(
            db,
            "external-auth-user",
            "external-auth-user@example.com",
            "password-hash",
            UserRole::User,
        )
        .await
        .expect("external auth test user should insert")
        .id
    }

    #[tokio::test]
    async fn provider_queries_filter_enabled_kind_and_paginate_by_display_name() {
        let db = build_test_db().await;
        let beta = insert_test_provider(&db, "beta", "Beta", ExternalAuthKind::Oidc, true).await;
        let alpha =
            insert_test_provider(&db, "alpha", "Alpha", ExternalAuthKind::Oauth2, true).await;
        let disabled =
            insert_test_provider(&db, "disabled", "Disabled", ExternalAuthKind::Oidc, false).await;

        let enabled = list_enabled_providers(&db).await.unwrap();
        assert_eq!(
            enabled
                .iter()
                .map(|provider| provider.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );

        let oidc = list_enabled_providers_by_kind(&db, ExternalAuthKind::Oidc)
            .await
            .unwrap();
        assert_eq!(
            oidc.iter().map(|provider| provider.id).collect::<Vec<_>>(),
            vec![beta.id]
        );

        let (page, total) = list_providers_paginated(&db, 2, 1).await.unwrap();
        assert_eq!(total, 3);
        assert_eq!(
            page.iter()
                .map(|provider| provider.slug.as_str())
                .collect::<Vec<_>>(),
            vec!["beta", "disabled"]
        );

        assert_eq!(
            find_provider_by_id(&db, alpha.id).await.unwrap().slug,
            "alpha"
        );
        assert_eq!(
            find_provider_by_slug(&db, "beta")
                .await
                .unwrap()
                .unwrap()
                .id,
            beta.id
        );
        assert_eq!(
            find_enabled_provider_by_slug(&db, "alpha")
                .await
                .unwrap()
                .id,
            alpha.id
        );
        assert!(
            find_enabled_provider_by_slug(&db, "disabled")
                .await
                .unwrap_err()
                .message()
                .contains("external auth provider disabled")
        );

        delete_provider(&db, disabled.id).await.unwrap();
        assert!(find_provider_by_id(&db, disabled.id).await.is_err());
        assert!(
            delete_provider(&db, disabled.id)
                .await
                .unwrap_err()
                .message()
                .contains("external auth provider")
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn login_flow_consume_and_cleanup_enforce_state_lifecycle() {
        let db = build_test_db().await;
        let provider =
            insert_test_provider(&db, "oidc", "OIDC", ExternalAuthKind::Oidc, true).await;
        let now = Utc::now();

        let flow = create_login_flow(
            &db,
            provider.id,
            "state-live",
            "https://app.example.com/callback",
            now + Duration::minutes(5),
        )
        .await
        .unwrap();
        assert_eq!(flow.consumed_at, None);
        assert_eq!(flow.redirect_uri, "https://app.example.com/callback");

        let consumed = consume_login_flow(&db, "state-live").await.unwrap();
        assert_eq!(consumed.id, flow.id);
        assert!(consumed.consumed_at.is_some());
        assert!(
            consume_login_flow(&db, "state-live")
                .await
                .unwrap_err()
                .message()
                .contains("expired or consumed")
        );
        assert!(
            consume_login_flow(&db, "state-missing")
                .await
                .unwrap_err()
                .message()
                .contains("invalid external auth state")
        );

        create_login_flow(
            &db,
            provider.id,
            "state-expired",
            "https://app.example.com/callback",
            now - Duration::minutes(1),
        )
        .await
        .unwrap();
        create_login_flow(
            &db,
            provider.id,
            "state-future",
            "https://app.example.com/callback",
            now + Duration::minutes(10),
        )
        .await
        .unwrap();
        assert!(
            consume_login_flow(&db, "state-expired")
                .await
                .unwrap_err()
                .message()
                .contains("expired or consumed")
        );

        assert_eq!(cleanup_expired_login_flows(&db, now).await.unwrap(), 1);
        assert!(
            consume_login_flow(&db, "state-expired")
                .await
                .unwrap_err()
                .message()
                .contains("invalid external auth state")
        );
        assert!(
            consume_login_flow(&db, "state-future")
                .await
                .unwrap()
                .consumed_at
                .is_some()
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn link_identity_persists_provider_subject_and_profile_fields() {
        let db = build_test_db().await;
        let provider =
            insert_test_provider(&db, "identity", "Identity", ExternalAuthKind::Oidc, true).await;
        let user_id = insert_user(&db).await;

        let identity = link_identity(
            &db,
            user_id,
            provider.id,
            "subject-123",
            Some("linked@example.com".to_string()),
            Some("Linked User".to_string()),
        )
        .await
        .unwrap();

        assert_eq!(identity.user_id, user_id);
        assert_eq!(identity.provider_id, provider.id);
        assert_eq!(identity.subject, "subject-123");
        assert_eq!(identity.email.as_deref(), Some("linked@example.com"));
        assert_eq!(identity.display_name.as_deref(), Some("Linked User"));

        let duplicate = link_identity(&db, user_id, provider.id, "subject-123", None, None)
            .await
            .unwrap_err();
        assert!(duplicate.message().contains("UNIQUE") || duplicate.message().contains("unique"));

        db.close().await.unwrap();
    }
}
