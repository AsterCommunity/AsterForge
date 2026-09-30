//! Database migrations for the generated Aster service.
//!
//! Forge owns the product-neutral infrastructure schema builders used here. Product-specific
//! migrations should be added as new modules in this crate and registered after the foundation
//! migration.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
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

pub use sea_orm_migration::prelude::*;

mod m20260627_000001_forge_foundation_schema;

/// Service migrator.
pub struct Migrator;

impl Migrator {
    /// Runs migrations up to the requested number of steps.
    pub async fn up(
        db: &sea_orm_migration::sea_orm::DatabaseConnection,
        steps: Option<u32>,
    ) -> Result<(), DbErr> {
        <Self as MigratorTrait>::up(db, steps).await
    }
}

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(
            m20260627_000001_forge_foundation_schema::Migration,
        )]
    }
}
