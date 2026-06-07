//! Auth session repository.

use crate::entities::auth_session::{self, Entity as AuthSession};
use crate::errors::{AsterError, MapAsterErr, Result};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};

pub async fn create<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
    refresh_token_hash: &str,
    session_version: i64,
    expires_at: chrono::DateTime<chrono::Utc>,
    user_agent: Option<String>,
    ip_address: Option<String>,
) -> Result<auth_session::Model> {
    auth_session::ActiveModel {
        user_id: Set(user_id),
        refresh_token_hash: Set(refresh_token_hash.to_string()),
        session_version: Set(session_version),
        user_agent: Set(user_agent),
        ip_address: Set(ip_address),
        expires_at: Set(expires_at),
        created_at: Set(chrono::Utc::now()),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_aster_err(AsterError::database_operation)
}

pub async fn find_active_by_refresh_hash<C: ConnectionTrait>(
    db: &C,
    hash: &str,
) -> Result<Option<auth_session::Model>> {
    AuthSession::find()
        .filter(auth_session::Column::RefreshTokenHash.eq(hash))
        .filter(auth_session::Column::RevokedAt.is_null())
        .one(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn list_by_user<C: ConnectionTrait>(
    db: &C,
    user_id: i64,
) -> Result<Vec<auth_session::Model>> {
    AuthSession::find()
        .filter(auth_session::Column::UserId.eq(user_id))
        .order_by_desc(auth_session::Column::CreatedAt)
        .all(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn revoke_by_refresh_hash<C: ConnectionTrait>(db: &C, hash: &str) -> Result<bool> {
    let Some(session) = find_active_by_refresh_hash(db, hash).await? else {
        return Ok(false);
    };
    let mut active: auth_session::ActiveModel = session.into();
    active.revoked_at = Set(Some(chrono::Utc::now()));
    active
        .update(db)
        .await
        .map_aster_err(AsterError::database_operation)?;
    Ok(true)
}

pub async fn delete_expired<C: ConnectionTrait>(
    db: &C,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<u64> {
    let result = AuthSession::delete_many()
        .filter(auth_session::Column::ExpiresAt.lt(now))
        .exec(db)
        .await
        .map_aster_err(AsterError::database_operation)?;
    Ok(result.rows_affected)
}

#[cfg(test)]
mod tests {
    use super::{
        create, delete_expired, find_active_by_refresh_hash, list_by_user, revoke_by_refresh_hash,
    };
    use crate::config::DatabaseConfig;
    use crate::db::repository::user_repo;
    use crate::types::UserRole;
    use chrono::{Duration, Utc};

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
        .expect("auth session repo test DB should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("auth session repo test migrations should succeed");
        db
    }

    async fn insert_user(db: &sea_orm::DatabaseConnection, suffix: &str) -> i64 {
        user_repo::create(
            db,
            &format!("session-user-{suffix}"),
            &format!("session-user-{suffix}@example.com"),
            "password-hash",
            UserRole::User,
        )
        .await
        .expect("auth session test user should insert")
        .id
    }

    #[tokio::test]
    async fn create_find_list_and_revoke_session_by_refresh_hash() {
        let db = build_test_db().await;
        let user_id = insert_user(&db, "flow").await;
        let expires_at = Utc::now() + Duration::hours(1);

        let first = create(
            &db,
            user_id,
            "refresh-hash-one",
            1,
            expires_at,
            Some("Firefox".to_string()),
            Some("127.0.0.1".to_string()),
        )
        .await
        .unwrap();
        let second = create(&db, user_id, "refresh-hash-two", 2, expires_at, None, None)
            .await
            .unwrap();

        let active = find_active_by_refresh_hash(&db, "refresh-hash-one")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.id, first.id);
        assert_eq!(active.user_agent.as_deref(), Some("Firefox"));
        assert_eq!(active.ip_address.as_deref(), Some("127.0.0.1"));

        let sessions = list_by_user(&db, user_id).await.unwrap();
        let session_ids = sessions
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();
        assert_eq!(session_ids.len(), 2);
        assert!(session_ids.contains(&first.id));
        assert!(session_ids.contains(&second.id));

        assert!(
            revoke_by_refresh_hash(&db, "refresh-hash-one")
                .await
                .unwrap()
        );
        assert!(
            find_active_by_refresh_hash(&db, "refresh-hash-one")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !revoke_by_refresh_hash(&db, "refresh-hash-one")
                .await
                .unwrap()
        );
        assert!(
            !revoke_by_refresh_hash(&db, "missing-refresh-hash")
                .await
                .unwrap()
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn delete_expired_removes_only_expired_sessions() {
        let db = build_test_db().await;
        let user_id = insert_user(&db, "cleanup").await;
        let now = Utc::now();
        let expired = create(
            &db,
            user_id,
            "expired-refresh-hash",
            1,
            now - Duration::seconds(1),
            None,
            None,
        )
        .await
        .unwrap();
        let active = create(
            &db,
            user_id,
            "active-refresh-hash",
            1,
            now + Duration::hours(1),
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(delete_expired(&db, now).await.unwrap(), 1);
        let sessions = list_by_user(&db, user_id).await.unwrap();
        assert_eq!(
            sessions
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![active.id]
        );
        assert!(
            find_active_by_refresh_hash(&db, &expired.refresh_token_hash)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            find_active_by_refresh_hash(&db, &active.refresh_token_hash)
                .await
                .unwrap()
                .is_some()
        );

        db.close().await.unwrap();
    }
}
