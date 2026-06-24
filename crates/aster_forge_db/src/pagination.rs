//! SeaORM repository helpers for offset pagination.
//!
//! The helper executes the same select query for total count and page items, letting service code
//! build filters once and reuse them consistently. It stays generic over entities and connections
//! so product repositories can keep their own model types.

use sea_orm::{ConnectionTrait, EntityTrait, PaginatorTrait, QuerySelect, Select};

use crate::{DbError, Result};

/// Fetches an offset page and total count from a SeaORM select query.
pub async fn fetch_offset_page<C, E>(
    db: &C,
    query: Select<E>,
    limit: u64,
    offset: u64,
) -> Result<(Vec<E::Model>, u64)>
where
    C: ConnectionTrait,
    E: EntityTrait,
    Select<E>: QuerySelect,
    for<'db> Select<E>: PaginatorTrait<'db, C>,
{
    let total = query.clone().count(db).await.map_err(DbError::from)?;
    let items = query
        .limit(limit)
        .offset(offset)
        .all(db)
        .await
        .map_err(DbError::from)?;
    Ok((items, total))
}

#[cfg(test)]
mod tests {
    use super::fetch_offset_page;
    use sea_orm::{
        ActiveModelBehavior, ConnectionTrait, Database, DeriveEntityModel, DerivePrimaryKey,
        DeriveRelation, EntityTrait, EnumIter, PrimaryKeyTrait, QueryOrder,
    };

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "pagination_items")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    #[tokio::test]
    async fn fetch_offset_page_returns_items_and_total_count() {
        let db = Database::connect("sqlite::memory:")
            .await
            .expect("sqlite memory database should connect");
        db.execute_unprepared(
            "CREATE TABLE pagination_items (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
        )
        .await
        .expect("table should be created");
        db.execute_unprepared(
            "INSERT INTO pagination_items (id, name) VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma');",
        )
        .await
        .expect("rows should be inserted");

        let (items, total) = fetch_offset_page(&db, Entity::find().order_by_asc(Column::Id), 2, 1)
            .await
            .expect("page should load");

        assert_eq!(total, 3);
        assert_eq!(
            items.into_iter().map(|item| item.name).collect::<Vec<_>>(),
            vec!["beta".to_string(), "gamma".to_string()]
        );
    }
}
