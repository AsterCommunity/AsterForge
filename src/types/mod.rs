//! Shared domain enums.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt;
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    #[sea_orm(string_value = "admin")]
    Admin,
    #[sea_orm(string_value = "user")]
    User,
}

impl UserRole {
    pub const fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    #[sea_orm(string_value = "active")]
    Active,
    #[sea_orm(string_value = "disabled")]
    Disabled,
}

impl UserStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum ExternalAuthKind {
    #[sea_orm(string_value = "oidc")]
    Oidc,
    #[sea_orm(string_value = "oauth2")]
    Oauth2,
}

impl ExternalAuthKind {
    pub const ALL: [Self; 2] = [Self::Oidc, Self::Oauth2];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Oidc => "oidc",
            Self::Oauth2 => "oauth2",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "oidc" => Some(Self::Oidc),
            "oauth2" => Some(Self::Oauth2),
            _ => None,
        }
    }
}

impl fmt::Display for ExternalAuthKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum SystemConfigValueType {
    #[sea_orm(string_value = "string")]
    String,
    #[sea_orm(string_value = "multiline")]
    Multiline,
    #[sea_orm(string_value = "string_array")]
    StringArray,
    #[sea_orm(string_value = "string_enum_set")]
    StringEnumSet,
    #[sea_orm(string_value = "number")]
    Number,
    #[sea_orm(string_value = "boolean")]
    Boolean,
}

impl SystemConfigValueType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Multiline => "multiline",
            Self::StringArray => "string_array",
            Self::StringEnumSet => "string_enum_set",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }

    pub fn from_str_name(value: &str) -> Option<Self> {
        match value {
            "string" => Some(Self::String),
            "multiline" => Some(Self::Multiline),
            "string_array" => Some(Self::StringArray),
            "string_enum_set" => Some(Self::StringEnumSet),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            _ => None,
        }
    }

    pub const fn is_multiline(self) -> bool {
        matches!(self, Self::Multiline)
    }

    pub const fn is_string_array(self) -> bool {
        matches!(self, Self::StringArray)
    }

    pub const fn is_string_enum_set(self) -> bool {
        matches!(self, Self::StringEnumSet)
    }

    pub const fn is_string_list(self) -> bool {
        matches!(self, Self::StringArray | Self::StringEnumSet)
    }
}

impl fmt::Display for SystemConfigValueType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SystemConfigSource {
    #[sea_orm(string_value = "system")]
    #[default]
    System,
    #[sea_orm(string_value = "custom")]
    Custom,
}

impl SystemConfigSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Custom => "custom",
        }
    }
}

impl fmt::Display for SystemConfigSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SystemConfigVisibility {
    #[sea_orm(string_value = "private")]
    #[default]
    Private,
    #[sea_orm(string_value = "public")]
    Public,
    #[sea_orm(string_value = "authenticated")]
    Authenticated,
}

impl SystemConfigVisibility {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
            Self::Authenticated => "authenticated",
        }
    }
}

impl fmt::Display for SystemConfigVisibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DeriveValueType)]
pub struct StoredTaskPayload(pub String);

impl AsRef<str> for StoredTaskPayload {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for StoredTaskPayload {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<StoredTaskPayload> for String {
    fn from(value: StoredTaskPayload) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DeriveValueType)]
pub struct StoredTaskResult(pub String);

impl AsRef<str> for StoredTaskResult {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for StoredTaskResult {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<StoredTaskResult> for String {
    fn from(value: StoredTaskResult) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DeriveValueType)]
pub struct StoredTaskRuntime(pub String);

impl AsRef<str> for StoredTaskRuntime {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for StoredTaskRuntime {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<StoredTaskRuntime> for String {
    fn from(value: StoredTaskRuntime) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, DeriveValueType)]
pub struct StoredTaskSteps(pub String);

impl AsRef<str> for StoredTaskSteps {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<String> for StoredTaskSteps {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<StoredTaskSteps> for String {
    fn from(value: StoredTaskSteps) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskKind {
    #[sea_orm(string_value = "system_runtime")]
    SystemRuntime,
}

impl BackgroundTaskKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SystemRuntime => "system_runtime",
        }
    }
}

impl fmt::Display for BackgroundTaskKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    #[sea_orm(string_value = "pending")]
    Pending,
    #[sea_orm(string_value = "processing")]
    Processing,
    #[sea_orm(string_value = "retry")]
    Retry,
    #[sea_orm(string_value = "succeeded")]
    Succeeded,
    #[sea_orm(string_value = "failed")]
    Failed,
    #[sea_orm(string_value = "canceled")]
    Canceled,
}

impl BackgroundTaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Retry => "retry",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }
}

impl fmt::Display for BackgroundTaskStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

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
            Self::ConfigUpdate | Self::ConfigDelete | Self::AdminDeleteConfig => "config",
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
