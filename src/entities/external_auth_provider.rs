//! External auth provider entity.

use crate::types::ExternalAuthKind;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
#[cfg(all(debug_assertions, feature = "openapi"))]
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(ToSchema))]
#[cfg_attr(
    all(debug_assertions, feature = "openapi"),
    schema(as = ExternalAuthProviderModel)
)]
#[sea_orm(table_name = "external_auth_providers")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    #[sea_orm(unique)]
    pub slug: String,
    pub display_name: String,
    pub kind: ExternalAuthKind,
    pub enabled: bool,
    pub issuer_url: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub userinfo_url: Option<String>,
    pub client_id: String,
    #[serde(skip_serializing)]
    pub client_secret: String,
    pub scopes: String,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub created_at: DateTimeUtc,
    #[cfg_attr(all(debug_assertions, feature = "openapi"), schema(value_type = String))]
    pub updated_at: DateTimeUtc,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::external_auth_identity::Entity")]
    Identities,
    #[sea_orm(has_many = "super::external_auth_login_flow::Entity")]
    LoginFlows,
}

impl Related<super::external_auth_identity::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Identities.def()
    }
}

impl Related<super::external_auth_login_flow::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::LoginFlows.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
