//! Shared repository helpers for applying whitelisted sort options.
//!
//! The helpers accept a caller-provided whitelist of field names and SeaORM columns, then apply
//! ascending or descending ordering to a query. Keeping this logic shared avoids repeating unsafe
//! ad-hoc string-to-column mapping in each repository.

use sea_orm::{ColumnTrait, QueryOrder};

pub use aster_forge_api::SortOrder;

/// Orders a query by one column.
pub fn order_by_column<Q, C>(query: Q, column: C, order: SortOrder) -> Q
where
    Q: QueryOrder,
    C: ColumnTrait,
{
    match order {
        SortOrder::Asc => query.order_by_asc(column),
        SortOrder::Desc => query.order_by_desc(column),
    }
}

/// Orders a query by one column and then by an id column for stable ordering.
pub fn order_by_column_with_id<E, C, I>(query: E, column: C, order: SortOrder, id_column: I) -> E
where
    E: QueryOrder,
    C: ColumnTrait,
    I: ColumnTrait,
{
    order_by_id(order_by_column(query, column, order), id_column, order)
}

/// Orders a query by an id column.
pub fn order_by_id<Q, I>(query: Q, id_column: I, order: SortOrder) -> Q
where
    Q: QueryOrder,
    I: ColumnTrait,
{
    match order {
        SortOrder::Asc => query.order_by_asc(id_column),
        SortOrder::Desc => query.order_by_desc(id_column),
    }
}

#[cfg(test)]
mod tests {
    use super::{SortOrder, order_by_column, order_by_column_with_id, order_by_id};
    use sea_orm::{
        ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
        EnumIter, PrimaryKeyTrait, QueryTrait,
    };

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "sortable_items")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub score: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}

    #[test]
    fn order_by_column_applies_requested_direction() {
        let asc_sql = order_by_column(Entity::find(), Column::Score, SortOrder::Asc)
            .build(sea_orm::DbBackend::Sqlite)
            .to_string();
        let desc_sql = order_by_column(Entity::find(), Column::Score, SortOrder::Desc)
            .build(sea_orm::DbBackend::Sqlite)
            .to_string();

        assert!(asc_sql.contains(r#"ORDER BY "sortable_items"."score" ASC"#));
        assert!(desc_sql.contains(r#"ORDER BY "sortable_items"."score" DESC"#));
    }

    #[test]
    fn order_by_column_with_id_adds_stable_tiebreaker() {
        let sql =
            order_by_column_with_id(Entity::find(), Column::Score, SortOrder::Desc, Column::Id)
                .build(sea_orm::DbBackend::Sqlite)
                .to_string();

        assert!(sql.contains(r#""sortable_items"."score" DESC"#));
        assert!(sql.contains(r#""sortable_items"."id" DESC"#));
    }

    #[test]
    fn order_by_id_orders_by_primary_key() {
        let sql = order_by_id(Entity::find(), Column::Id, SortOrder::Asc)
            .build(sea_orm::DbBackend::Sqlite)
            .to_string();

        assert!(sql.contains(r#"ORDER BY "sortable_items"."id" ASC"#));
    }
}
