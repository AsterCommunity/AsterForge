use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

macro_rules! define_audit_actions {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
        #[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
        #[sea_orm(rs_type = "String", db_type = "String(StringLen::N(64))")]
        #[serde(rename_all = "snake_case")]
        pub enum AuditAction {
            $(
                #[sea_orm(string_value = $name)]
                #[serde(rename = $name)]
                $variant,
            )+
        }

        impl AuditAction {
            pub const COUNT: usize = <[()]>::len(&[$(define_audit_actions!(@unit $variant)),+]);
            pub const ALL: [Self; Self::COUNT] = [$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            pub fn from_str_name(value: &str) -> Option<Self> {
                match value {
                    $($name => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub fn index(self) -> usize {
                Self::ALL
                    .iter()
                    .position(|action| *action == self)
                    .expect("audit action should be present in AuditAction::ALL")
            }
        }
    };
    (@unit $variant:ident) => { () };
}

define_audit_actions! {
    SystemSetup => "system_setup",
    ServerStart => "server_start",
    ServerShutdown => "server_shutdown",
    ConfigUpdate => "config_update",
    ConfigDelete => "config_delete",
    ConfigActionExecute => "config_action_execute",
    UserRegister => "user_register",
    UserLogin => "user_login",
    UserLogout => "user_logout",
    UserRefreshToken => "user_refresh_token",
    UserRevokeSession => "user_revoke_session",
    UserRevokeOtherSessions => "user_revoke_other_sessions",
    UserChangePassword => "user_change_password",
    UserUpdateProfile => "user_update_profile",
    AdminCreateUser => "admin_create_user",
    AdminUpdateUser => "admin_update_user",
    AdminDisableUser => "admin_disable_user",
    AdminRevokeUserSessions => "admin_revoke_user_sessions",
    AdminDeleteConfig => "admin_delete_config",
    AdminCleanupTasks => "admin_cleanup_tasks",
    TaskRetry => "task_retry",
    AdminCreateExternalAuthProvider => "admin_create_external_auth_provider",
    AdminUpdateExternalAuthProvider => "admin_update_external_auth_provider",
    AdminDeleteExternalAuthProvider => "admin_delete_external_auth_provider",
    AdminTestExternalAuthProvider => "admin_test_external_auth_provider",
    MailSend => "mail_send",
    MailDeliveryFailed => "mail_delivery_failed",
    ExternalAuthProviderCreate => "external_auth_provider_create",
    ExternalAuthProviderUpdate => "external_auth_provider_update",
    ExternalAuthProviderDelete => "external_auth_provider_delete",
    UserExternalAuthLogin => "user_external_auth_login",
    UserExternalAuthLink => "user_external_auth_link",
    UserExternalAuthUnlink => "user_external_auth_unlink",
}

impl AsRef<str> for AuditAction {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AuditAction {
    pub const fn group(self) -> &'static str {
        match self {
            Self::SystemSetup | Self::ServerStart | Self::ServerShutdown => "system",
            Self::ConfigUpdate
            | Self::ConfigDelete
            | Self::ConfigActionExecute
            | Self::AdminDeleteConfig => "config",
            Self::AdminCleanupTasks | Self::TaskRetry => "task",
            Self::UserRegister
            | Self::UserLogin
            | Self::UserLogout
            | Self::UserRefreshToken
            | Self::UserRevokeSession
            | Self::UserRevokeOtherSessions
            | Self::UserChangePassword
            | Self::UserUpdateProfile => "user",
            Self::AdminCreateUser
            | Self::AdminUpdateUser
            | Self::AdminDisableUser
            | Self::AdminRevokeUserSessions => "admin",
            Self::MailSend | Self::MailDeliveryFailed => "mail",
            Self::AdminCreateExternalAuthProvider
            | Self::AdminUpdateExternalAuthProvider
            | Self::AdminDeleteExternalAuthProvider
            | Self::AdminTestExternalAuthProvider
            | Self::ExternalAuthProviderCreate
            | Self::ExternalAuthProviderUpdate
            | Self::ExternalAuthProviderDelete
            | Self::UserExternalAuthLogin
            | Self::UserExternalAuthLink
            | Self::UserExternalAuthUnlink => "external_auth",
        }
    }
}

impl fmt::Display for AuditAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AuditEntityType {
    System,
    SystemConfig,
    User,
    AuthSession,
    ExternalAuthProvider,
    ExternalAuthIdentity,
    ApiToken,
    Mail,
    Task,
}

impl AuditEntityType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::SystemConfig => "system_config",
            Self::User => "user",
            Self::AuthSession => "auth_session",
            Self::ExternalAuthProvider => "external_auth_provider",
            Self::ExternalAuthIdentity => "external_auth_identity",
            Self::ApiToken => "api_token",
            Self::Mail => "mail",
            Self::Task => "task",
        }
    }

    pub fn from_str_name(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "system_config" => Some(Self::SystemConfig),
            "user" => Some(Self::User),
            "auth_session" => Some(Self::AuthSession),
            "external_auth_provider" => Some(Self::ExternalAuthProvider),
            "external_auth_identity" => Some(Self::ExternalAuthIdentity),
            "api_token" => Some(Self::ApiToken),
            "mail" => Some(Self::Mail),
            "task" => Some(Self::Task),
            _ => None,
        }
    }
}

impl AsRef<str> for AuditEntityType {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for AuditEntityType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
