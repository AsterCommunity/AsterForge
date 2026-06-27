//! Migration smoke tests.

use sea_orm_migration::prelude::MigratorTrait;

#[test]
fn foundation_migration_is_registered_first() {
    let migrations = migration::Migrator::migrations();

    assert_eq!(migrations.len(), 1);
    assert_eq!(
        migrations[0].name(),
        "m20260627_000001_forge_foundation_schema"
    );
}
