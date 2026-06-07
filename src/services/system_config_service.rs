//! Compatibility wrapper for the config service.

use crate::errors::Result;
use sea_orm::ConnectionTrait;

pub async fn ensure_defaults<C: ConnectionTrait>(db: &C) -> Result<()> {
    crate::services::config_service::ensure_defaults(db).await
}
