//! Forge-owned infrastructure tables used by the generated service.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_runtime_leases(manager).await?;
        create_scheduled_tasks(manager).await?;
        create_system_config(manager).await?;
        create_mail_outbox(manager).await?;
        create_audit_logs(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        drop_audit_logs(manager).await?;
        drop_mail_outbox(manager).await?;
        drop_system_config(manager).await?;
        drop_scheduled_tasks(manager).await?;
        drop_runtime_leases(manager).await?;
        Ok(())
    }
}

async fn create_runtime_leases(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(aster_forge_db::create_runtime_leases_table(
            manager.get_database_backend(),
        ))
        .await
}

async fn drop_runtime_leases(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_table(aster_forge_db::drop_runtime_leases_table())
        .await
}

async fn create_scheduled_tasks(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(aster_forge_db::create_scheduled_tasks_table(
            manager.get_database_backend(),
        ))
        .await?;
    manager
        .create_index(aster_forge_db::create_scheduled_tasks_namespace_name_unique_index())
        .await?;
    manager
        .create_index(aster_forge_db::create_scheduled_tasks_next_run_index())
        .await
}

async fn drop_scheduled_tasks(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_index(aster_forge_db::drop_scheduled_tasks_next_run_index())
        .await?;
    manager
        .drop_index(aster_forge_db::drop_scheduled_tasks_namespace_name_unique_index())
        .await?;
    manager
        .drop_table(aster_forge_db::drop_scheduled_tasks_table())
        .await
}

async fn create_system_config(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(aster_forge_db::create_system_config_table(
            manager.get_database_backend(),
        ))
        .await?;
    manager
        .create_index(aster_forge_db::create_system_config_key_unique_index())
        .await
}

async fn drop_system_config(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_index(aster_forge_db::drop_system_config_key_unique_index())
        .await?;
    manager
        .drop_table(aster_forge_db::drop_system_config_table())
        .await
}

async fn create_mail_outbox(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(aster_forge_db::create_mail_outbox_table(
            manager.get_database_backend(),
        ))
        .await?;
    manager
        .create_index(aster_forge_db::create_mail_outbox_due_index())
        .await?;
    manager
        .create_index(aster_forge_db::create_mail_outbox_processing_index())
        .await?;
    manager
        .create_index(aster_forge_db::create_mail_outbox_sent_at_index())
        .await
}

async fn drop_mail_outbox(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .drop_index(aster_forge_db::drop_mail_outbox_sent_at_index())
        .await?;
    manager
        .drop_index(aster_forge_db::drop_mail_outbox_processing_index())
        .await?;
    manager
        .drop_index(aster_forge_db::drop_mail_outbox_due_index())
        .await?;
    manager
        .drop_table(aster_forge_db::drop_mail_outbox_table())
        .await
}

async fn create_audit_logs(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(aster_forge_db::create_audit_logs_table(
            manager.get_database_backend(),
        ))
        .await?;
    for index in aster_forge_db::create_audit_logs_base_indexes() {
        manager.create_index(index).await?;
    }
    for index in aster_forge_db::create_audit_logs_query_indexes() {
        manager.create_index(index).await?;
    }
    Ok(())
}

async fn drop_audit_logs(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    for drop_index in [
        aster_forge_db::drop_audit_logs_entity_type_created_id_index(),
        aster_forge_db::drop_audit_logs_action_created_id_index(),
        aster_forge_db::drop_audit_logs_user_created_id_index(),
        aster_forge_db::drop_audit_logs_created_id_index(),
        aster_forge_db::drop_audit_logs_action_created_user_index(),
        aster_forge_db::drop_audit_logs_user_id_index(),
        aster_forge_db::drop_audit_logs_action_index(),
        aster_forge_db::drop_audit_logs_created_at_index(),
    ] {
        manager.drop_index(drop_index).await?;
    }
    manager
        .drop_table(aster_forge_db::drop_audit_logs_table())
        .await
}
