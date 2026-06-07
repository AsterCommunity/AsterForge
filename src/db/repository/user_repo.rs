//! User repository.

use crate::entities::user::{self, Entity as User};
use crate::errors::{AsterError, MapAsterErr, Result};
use crate::types::{UserRole, UserStatus};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, Set,
};

pub async fn count_all<C: ConnectionTrait>(db: &C) -> Result<u64> {
    User::find()
        .count(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn find_by_id<C: ConnectionTrait>(db: &C, id: i64) -> Result<user::Model> {
    User::find_by_id(id)
        .one(db)
        .await
        .map_aster_err(AsterError::database_operation)?
        .ok_or_else(|| AsterError::record_not_found(format!("user #{id}")))
}

pub async fn find_by_ids<C: ConnectionTrait>(db: &C, ids: &[i64]) -> Result<Vec<user::Model>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    User::find()
        .filter(user::Column::Id.is_in(ids.iter().copied()))
        .order_by_asc(user::Column::Id)
        .all(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn find_by_identifier<C: ConnectionTrait>(
    db: &C,
    identifier: &str,
) -> Result<Option<user::Model>> {
    User::find()
        .filter(
            sea_orm::Condition::any()
                .add(user::Column::Username.eq(identifier))
                .add(user::Column::Email.eq(identifier)),
        )
        .one(db)
        .await
        .map_aster_err(AsterError::database_operation)
}

pub async fn create<C: ConnectionTrait>(
    db: &C,
    username: &str,
    email: &str,
    password_hash: &str,
    role: UserRole,
) -> Result<user::Model> {
    let now = chrono::Utc::now();
    user::ActiveModel {
        username: Set(username.to_string()),
        email: Set(email.to_string()),
        password_hash: Set(password_hash.to_string()),
        role: Set(role),
        status: Set(UserStatus::Active),
        session_version: Set(1),
        email_verified_at: Set(Some(now)),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .map_aster_err(AsterError::database_operation)
}

pub async fn bump_session_version<C: ConnectionTrait>(db: &C, user_id: i64) -> Result<()> {
    let user = find_by_id(db, user_id).await?;
    let mut active: user::ActiveModel = user.into();
    active.session_version = Set(active.session_version.unwrap() + 1);
    active.updated_at = Set(chrono::Utc::now());
    active
        .update(db)
        .await
        .map_aster_err(AsterError::database_operation)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        bump_session_version, count_all, create, find_by_id, find_by_identifier, find_by_ids,
    };
    use crate::config::DatabaseConfig;
    use crate::types::{UserRole, UserStatus};

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
        .expect("user repo test DB should connect");
        migration::Migrator::up(&db, None)
            .await
            .expect("user repo test migrations should succeed");
        db
    }

    #[tokio::test]
    async fn create_count_find_and_bump_session_version() {
        let db = build_test_db().await;
        assert_eq!(count_all(&db).await.unwrap(), 0);

        let user = create(
            &db,
            "repo-user",
            "repo-user@example.com",
            "password-hash",
            UserRole::Admin,
        )
        .await
        .unwrap();

        assert_eq!(count_all(&db).await.unwrap(), 1);
        assert_eq!(user.username, "repo-user");
        assert_eq!(user.email, "repo-user@example.com");
        assert_eq!(user.role, UserRole::Admin);
        assert_eq!(user.status, UserStatus::Active);
        assert_eq!(user.session_version, 1);
        assert!(user.email_verified_at.is_some());

        assert_eq!(
            find_by_id(&db, user.id).await.unwrap().username,
            "repo-user"
        );
        assert_eq!(
            find_by_identifier(&db, "repo-user")
                .await
                .unwrap()
                .unwrap()
                .id,
            user.id
        );
        assert_eq!(
            find_by_identifier(&db, "repo-user@example.com")
                .await
                .unwrap()
                .unwrap()
                .id,
            user.id
        );
        assert!(find_by_identifier(&db, "missing").await.unwrap().is_none());

        bump_session_version(&db, user.id).await.unwrap();
        let bumped = find_by_id(&db, user.id).await.unwrap();
        assert_eq!(bumped.session_version, 2);
        assert!(bumped.updated_at >= user.updated_at);

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn find_by_ids_returns_empty_for_empty_input_and_orders_by_id() {
        let db = build_test_db().await;
        assert!(find_by_ids(&db, &[]).await.unwrap().is_empty());

        let first = create(
            &db,
            "repo-user-a",
            "repo-user-a@example.com",
            "password-hash",
            UserRole::User,
        )
        .await
        .unwrap();
        let second = create(
            &db,
            "repo-user-b",
            "repo-user-b@example.com",
            "password-hash",
            UserRole::User,
        )
        .await
        .unwrap();
        let third = create(
            &db,
            "repo-user-c",
            "repo-user-c@example.com",
            "password-hash",
            UserRole::User,
        )
        .await
        .unwrap();

        let users = find_by_ids(&db, &[third.id, first.id, 999_999, second.id])
            .await
            .unwrap();
        assert_eq!(
            users.into_iter().map(|user| user.id).collect::<Vec<_>>(),
            vec![first.id, second.id, third.id]
        );

        db.close().await.unwrap();
    }

    #[tokio::test]
    async fn find_by_id_and_bump_missing_user_return_not_found() {
        let db = build_test_db().await;

        let missing = find_by_id(&db, 404).await.unwrap_err();
        assert!(missing.message().contains("user #404"));
        assert!(
            bump_session_version(&db, 404)
                .await
                .unwrap_err()
                .message()
                .contains("user #404")
        );

        db.close().await.unwrap();
    }
}
