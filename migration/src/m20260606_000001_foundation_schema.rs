//! Foundation schema for AsterForge templates.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_users(manager).await?;
        create_auth_sessions(manager).await?;
        create_external_auth_providers(manager).await?;
        create_external_auth_identities(manager).await?;
        create_external_auth_login_flows(manager).await?;
        create_system_config(manager).await?;
        create_audit_logs(manager).await?;
        create_background_tasks(manager).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for table in [
            BackgroundTasks::Table.into_iden(),
            AuditLogs::Table.into_iden(),
            SystemConfig::Table.into_iden(),
            ExternalAuthLoginFlows::Table.into_iden(),
            ExternalAuthIdentities::Table.into_iden(),
            ExternalAuthProviders::Table.into_iden(),
            AuthSessions::Table.into_iden(),
            Users::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(table).if_exists().to_owned())
                .await?;
        }
        Ok(())
    }
}

fn big_integer_pk<T: IntoIden>(column: T) -> ColumnDef {
    let mut column = ColumnDef::new(column);
    column
        .big_integer()
        .not_null()
        .auto_increment()
        .primary_key();
    column
}

fn utc_timestamp<T: IntoIden>(manager: &SchemaManager<'_>, column: T) -> ColumnDef {
    crate::time::utc_date_time_column(manager, column)
}

async fn create_users(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(Users::Table)
                .if_not_exists()
                .col(big_integer_pk(Users::Id))
                .col(ColumnDef::new(Users::Username).string_len(128).not_null())
                .col(ColumnDef::new(Users::Email).string_len(255).not_null())
                .col(
                    ColumnDef::new(Users::PasswordHash)
                        .string_len(255)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(Users::Role)
                        .string_len(32)
                        .not_null()
                        .default("user"),
                )
                .col(
                    ColumnDef::new(Users::Status)
                        .string_len(32)
                        .not_null()
                        .default("active"),
                )
                .col(
                    ColumnDef::new(Users::SessionVersion)
                        .big_integer()
                        .not_null()
                        .default(1),
                )
                .col(utc_timestamp(manager, Users::EmailVerifiedAt).null())
                .col(utc_timestamp(manager, Users::CreatedAt).not_null())
                .col(utc_timestamp(manager, Users::UpdatedAt).not_null())
                .index(
                    Index::create()
                        .name("idx_users_username_unique")
                        .col(Users::Username)
                        .unique(),
                )
                .index(
                    Index::create()
                        .name("idx_users_email_unique")
                        .col(Users::Email)
                        .unique(),
                )
                .to_owned(),
        )
        .await
}

async fn create_auth_sessions(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(AuthSessions::Table)
                .if_not_exists()
                .col(big_integer_pk(AuthSessions::Id))
                .col(
                    ColumnDef::new(AuthSessions::UserId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AuthSessions::RefreshTokenHash)
                        .string_len(128)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AuthSessions::SessionVersion)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(AuthSessions::UserAgent)
                        .string_len(512)
                        .null(),
                )
                .col(
                    ColumnDef::new(AuthSessions::IpAddress)
                        .string_len(128)
                        .null(),
                )
                .col(utc_timestamp(manager, AuthSessions::ExpiresAt).not_null())
                .col(utc_timestamp(manager, AuthSessions::RevokedAt).null())
                .col(utc_timestamp(manager, AuthSessions::CreatedAt).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_auth_sessions_user")
                        .from(AuthSessions::Table, AuthSessions::UserId)
                        .to(Users::Table, Users::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .index(
                    Index::create()
                        .name("idx_auth_sessions_refresh_hash_unique")
                        .col(AuthSessions::RefreshTokenHash)
                        .unique(),
                )
                .to_owned(),
        )
        .await
}

async fn create_external_auth_providers(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ExternalAuthProviders::Table)
                .if_not_exists()
                .col(big_integer_pk(ExternalAuthProviders::Id))
                .col(
                    ColumnDef::new(ExternalAuthProviders::Slug)
                        .string_len(96)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::DisplayName)
                        .string_len(128)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::Kind)
                        .string_len(32)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::Enabled)
                        .boolean()
                        .not_null()
                        .default(true),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::IssuerUrl)
                        .string_len(512)
                        .null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::AuthorizeUrl)
                        .string_len(512)
                        .null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::TokenUrl)
                        .string_len(512)
                        .null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::UserinfoUrl)
                        .string_len(512)
                        .null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::ClientId)
                        .string_len(255)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::ClientSecret)
                        .string_len(512)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthProviders::Scopes)
                        .string_len(255)
                        .not_null()
                        .default("openid profile email"),
                )
                .col(utc_timestamp(manager, ExternalAuthProviders::CreatedAt).not_null())
                .col(utc_timestamp(manager, ExternalAuthProviders::UpdatedAt).not_null())
                .index(
                    Index::create()
                        .name("idx_external_auth_providers_slug_unique")
                        .col(ExternalAuthProviders::Slug)
                        .unique(),
                )
                .to_owned(),
        )
        .await
}

async fn create_external_auth_identities(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ExternalAuthIdentities::Table)
                .if_not_exists()
                .col(big_integer_pk(ExternalAuthIdentities::Id))
                .col(
                    ColumnDef::new(ExternalAuthIdentities::UserId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthIdentities::ProviderId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthIdentities::Subject)
                        .string_len(255)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthIdentities::Email)
                        .string_len(255)
                        .null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthIdentities::DisplayName)
                        .string_len(255)
                        .null(),
                )
                .col(utc_timestamp(manager, ExternalAuthIdentities::LinkedAt).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_external_auth_identities_user")
                        .from(
                            ExternalAuthIdentities::Table,
                            ExternalAuthIdentities::UserId,
                        )
                        .to(Users::Table, Users::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_external_auth_identities_provider")
                        .from(
                            ExternalAuthIdentities::Table,
                            ExternalAuthIdentities::ProviderId,
                        )
                        .to(ExternalAuthProviders::Table, ExternalAuthProviders::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .index(
                    Index::create()
                        .name("idx_external_auth_identity_unique")
                        .col(ExternalAuthIdentities::ProviderId)
                        .col(ExternalAuthIdentities::Subject)
                        .unique(),
                )
                .to_owned(),
        )
        .await
}

async fn create_external_auth_login_flows(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(ExternalAuthLoginFlows::Table)
                .if_not_exists()
                .col(big_integer_pk(ExternalAuthLoginFlows::Id))
                .col(
                    ColumnDef::new(ExternalAuthLoginFlows::ProviderId)
                        .big_integer()
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthLoginFlows::State)
                        .string_len(128)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(ExternalAuthLoginFlows::RedirectUri)
                        .string_len(1024)
                        .not_null(),
                )
                .col(utc_timestamp(manager, ExternalAuthLoginFlows::ExpiresAt).not_null())
                .col(utc_timestamp(manager, ExternalAuthLoginFlows::ConsumedAt).null())
                .col(utc_timestamp(manager, ExternalAuthLoginFlows::CreatedAt).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_external_auth_login_flows_provider")
                        .from(
                            ExternalAuthLoginFlows::Table,
                            ExternalAuthLoginFlows::ProviderId,
                        )
                        .to(ExternalAuthProviders::Table, ExternalAuthProviders::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .index(
                    Index::create()
                        .name("idx_external_auth_login_flows_state_unique")
                        .col(ExternalAuthLoginFlows::State)
                        .unique(),
                )
                .to_owned(),
        )
        .await
}

async fn create_system_config(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(SystemConfig::Table)
                .if_not_exists()
                .col(big_integer_pk(SystemConfig::Id))
                .col(ColumnDef::new(SystemConfig::Key).string_len(128).not_null())
                .col(ColumnDef::new(SystemConfig::Value).text().not_null())
                .col(
                    ColumnDef::new(SystemConfig::ValueType)
                        .string_len(32)
                        .not_null()
                        .default("string"),
                )
                .col(
                    ColumnDef::new(SystemConfig::RequiresRestart)
                        .boolean()
                        .not_null()
                        .default(false),
                )
                .col(
                    ColumnDef::new(SystemConfig::IsSensitive)
                        .boolean()
                        .not_null()
                        .default(false),
                )
                .col(
                    ColumnDef::new(SystemConfig::Source)
                        .string_len(16)
                        .not_null()
                        .default("system"),
                )
                .col(
                    ColumnDef::new(SystemConfig::Visibility)
                        .string_len(16)
                        .not_null()
                        .default("private"),
                )
                .col(
                    ColumnDef::new(SystemConfig::Namespace)
                        .string_len(64)
                        .not_null()
                        .default(""),
                )
                .col(
                    ColumnDef::new(SystemConfig::Category)
                        .string_len(64)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(SystemConfig::Description)
                        .string_len(512)
                        .not_null(),
                )
                .col(utc_timestamp(manager, SystemConfig::UpdatedAt).not_null())
                .col(ColumnDef::new(SystemConfig::UpdatedBy).big_integer().null())
                .index(
                    Index::create()
                        .name("idx_system_config_key_unique")
                        .col(SystemConfig::Key)
                        .unique(),
                )
                .to_owned(),
        )
        .await
}

async fn create_audit_logs(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(AuditLogs::Table)
                .if_not_exists()
                .col(big_integer_pk(AuditLogs::Id))
                .col(
                    ColumnDef::new(AuditLogs::UserId)
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .col(ColumnDef::new(AuditLogs::Action).string_len(64).not_null())
                .col(
                    ColumnDef::new(AuditLogs::EntityType)
                        .string_len(64)
                        .not_null(),
                )
                .col(ColumnDef::new(AuditLogs::EntityId).big_integer().null())
                .col(ColumnDef::new(AuditLogs::EntityName).string_len(255).null())
                .col(ColumnDef::new(AuditLogs::Details).text().null())
                .col(ColumnDef::new(AuditLogs::IpAddress).string_len(128).null())
                .col(ColumnDef::new(AuditLogs::UserAgent).string_len(512).null())
                .col(utc_timestamp(manager, AuditLogs::CreatedAt).not_null())
                .to_owned(),
        )
        .await?;

    for index in [
        Index::create()
            .name("idx_audit_logs_created_at")
            .table(AuditLogs::Table)
            .col(AuditLogs::CreatedAt)
            .to_owned(),
        Index::create()
            .name("idx_audit_logs_action")
            .table(AuditLogs::Table)
            .col(AuditLogs::Action)
            .to_owned(),
        Index::create()
            .name("idx_audit_logs_user_id")
            .table(AuditLogs::Table)
            .col(AuditLogs::UserId)
            .to_owned(),
    ] {
        manager.create_index(index).await?;
    }

    Ok(())
}

async fn create_background_tasks(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    manager
        .create_table(
            Table::create()
                .table(BackgroundTasks::Table)
                .if_not_exists()
                .col(big_integer_pk(BackgroundTasks::Id))
                .col(
                    ColumnDef::new(BackgroundTasks::Kind)
                        .string_len(32)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::Status)
                        .string_len(16)
                        .not_null()
                        .default("pending"),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::CreatorUserId)
                        .big_integer()
                        .null(),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::DisplayName)
                        .string_len(512)
                        .not_null(),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::PayloadJson)
                        .text()
                        .not_null(),
                )
                .col(ColumnDef::new(BackgroundTasks::ResultJson).text().null())
                .col(ColumnDef::new(BackgroundTasks::RuntimeJson).text().null())
                .col(ColumnDef::new(BackgroundTasks::StepsJson).text().null())
                .col(
                    ColumnDef::new(BackgroundTasks::ProgressCurrent)
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::ProgressTotal)
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::StatusText)
                        .string_len(255)
                        .null(),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::AttemptCount)
                        .integer()
                        .not_null()
                        .default(0),
                )
                .col(
                    ColumnDef::new(BackgroundTasks::MaxAttempts)
                        .integer()
                        .not_null()
                        .default(1),
                )
                .col(utc_timestamp(manager, BackgroundTasks::NextRunAt).not_null())
                .col(
                    ColumnDef::new(BackgroundTasks::ProcessingToken)
                        .big_integer()
                        .not_null()
                        .default(0),
                )
                .col(utc_timestamp(manager, BackgroundTasks::ProcessingStartedAt).null())
                .col(utc_timestamp(manager, BackgroundTasks::LastHeartbeatAt).null())
                .col(utc_timestamp(manager, BackgroundTasks::LeaseExpiresAt).null())
                .col(utc_timestamp(manager, BackgroundTasks::StartedAt).null())
                .col(utc_timestamp(manager, BackgroundTasks::FinishedAt).null())
                .col(ColumnDef::new(BackgroundTasks::LastError).text().null())
                .col(
                    ColumnDef::new(BackgroundTasks::FailureCanRetry)
                        .boolean()
                        .null(),
                )
                .col(utc_timestamp(manager, BackgroundTasks::ExpiresAt).not_null())
                .col(utc_timestamp(manager, BackgroundTasks::CreatedAt).not_null())
                .col(utc_timestamp(manager, BackgroundTasks::UpdatedAt).not_null())
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_background_tasks_creator_user")
                        .from(BackgroundTasks::Table, BackgroundTasks::CreatorUserId)
                        .to(Users::Table, Users::Id)
                        .on_delete(ForeignKeyAction::SetNull),
                )
                .to_owned(),
        )
        .await?;

    for index in [
        Index::create()
            .name("idx_background_tasks_status_next_run")
            .table(BackgroundTasks::Table)
            .col(BackgroundTasks::Status)
            .col(BackgroundTasks::NextRunAt)
            .to_owned(),
        Index::create()
            .name("idx_background_tasks_kind_status")
            .table(BackgroundTasks::Table)
            .col(BackgroundTasks::Kind)
            .col(BackgroundTasks::Status)
            .to_owned(),
        Index::create()
            .name("idx_background_tasks_lease_expires")
            .table(BackgroundTasks::Table)
            .col(BackgroundTasks::LeaseExpiresAt)
            .to_owned(),
        Index::create()
            .name("idx_background_tasks_expires_at")
            .table(BackgroundTasks::Table)
            .col(BackgroundTasks::ExpiresAt)
            .to_owned(),
        Index::create()
            .name("idx_background_tasks_updated_at")
            .table(BackgroundTasks::Table)
            .col(BackgroundTasks::UpdatedAt)
            .to_owned(),
    ] {
        manager.create_index(index).await?;
    }

    Ok(())
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
    Username,
    Email,
    PasswordHash,
    Role,
    Status,
    SessionVersion,
    EmailVerifiedAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum AuthSessions {
    Table,
    Id,
    UserId,
    RefreshTokenHash,
    SessionVersion,
    UserAgent,
    IpAddress,
    ExpiresAt,
    RevokedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum ExternalAuthProviders {
    Table,
    Id,
    Slug,
    DisplayName,
    Kind,
    Enabled,
    IssuerUrl,
    AuthorizeUrl,
    TokenUrl,
    UserinfoUrl,
    ClientId,
    ClientSecret,
    Scopes,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum ExternalAuthIdentities {
    Table,
    Id,
    UserId,
    ProviderId,
    Subject,
    Email,
    DisplayName,
    LinkedAt,
}

#[derive(DeriveIden)]
enum ExternalAuthLoginFlows {
    Table,
    Id,
    ProviderId,
    State,
    RedirectUri,
    ExpiresAt,
    ConsumedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum SystemConfig {
    Table,
    Id,
    Key,
    Value,
    ValueType,
    RequiresRestart,
    IsSensitive,
    Source,
    Visibility,
    Namespace,
    Category,
    Description,
    UpdatedAt,
    UpdatedBy,
}

#[derive(DeriveIden)]
enum AuditLogs {
    Table,
    Id,
    UserId,
    Action,
    EntityType,
    EntityId,
    EntityName,
    Details,
    IpAddress,
    UserAgent,
    CreatedAt,
}

#[derive(DeriveIden)]
enum BackgroundTasks {
    Table,
    Id,
    Kind,
    Status,
    CreatorUserId,
    DisplayName,
    PayloadJson,
    ResultJson,
    RuntimeJson,
    StepsJson,
    ProgressCurrent,
    ProgressTotal,
    StatusText,
    AttemptCount,
    MaxAttempts,
    NextRunAt,
    ProcessingToken,
    ProcessingStartedAt,
    LastHeartbeatAt,
    LeaseExpiresAt,
    StartedAt,
    FinishedAt,
    LastError,
    FailureCanRetry,
    ExpiresAt,
    CreatedAt,
    UpdatedAt,
}
