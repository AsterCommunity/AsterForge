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
    aster_forge_db::drop_index_if_exists(
        manager.get_connection(),
        aster_forge_db::SCHEDULED_TASKS_TABLE,
        aster_forge_db::SCHEDULED_TASK_NEXT_RUN_INDEX,
    )
    .await?;
    aster_forge_db::drop_index_if_exists(
        manager.get_connection(),
        aster_forge_db::SCHEDULED_TASKS_TABLE,
        aster_forge_db::SCHEDULED_TASK_NAMESPACE_NAME_UNIQUE_INDEX,
    )
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
    aster_forge_db::drop_index_if_exists(
        manager.get_connection(),
        aster_forge_db::SYSTEM_CONFIG_TABLE,
        aster_forge_db::SYSTEM_CONFIG_KEY_UNIQUE_INDEX,
    )
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
    for index_name in [
        aster_forge_db::MAIL_OUTBOX_SENT_AT_INDEX,
        aster_forge_db::MAIL_OUTBOX_PROCESSING_INDEX,
        aster_forge_db::MAIL_OUTBOX_DUE_INDEX,
    ] {
        aster_forge_db::drop_index_if_exists(
            manager.get_connection(),
            aster_forge_db::MAIL_OUTBOX_TABLE,
            index_name,
        )
        .await?;
    }
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
    for index_name in [
        aster_forge_db::AUDIT_LOG_ENTITY_TYPE_CREATED_ID_INDEX,
        aster_forge_db::AUDIT_LOG_ACTION_CREATED_ID_INDEX,
        aster_forge_db::AUDIT_LOG_USER_CREATED_ID_INDEX,
        aster_forge_db::AUDIT_LOG_CREATED_ID_INDEX,
        aster_forge_db::AUDIT_LOG_ACTION_CREATED_USER_INDEX,
        aster_forge_db::AUDIT_LOG_USER_ID_INDEX,
        aster_forge_db::AUDIT_LOG_ACTION_INDEX,
        aster_forge_db::AUDIT_LOG_CREATED_AT_INDEX,
    ] {
        aster_forge_db::drop_index_if_exists(
            manager.get_connection(),
            aster_forge_db::AUDIT_LOGS_TABLE,
            index_name,
        )
        .await?;
    }
    manager
        .drop_table(aster_forge_db::drop_audit_logs_table())
        .await
}
