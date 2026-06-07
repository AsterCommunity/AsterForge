use std::sync::Arc;

use crate::config::Config;
use crate::errors::Result;
use crate::runtime::AppState;

use super::common::prepare_common;

pub struct PreparedPrimaryRuntime {
    pub state: AppState,
}

pub async fn prepare_primary(config: Arc<Config>) -> Result<PreparedPrimaryRuntime> {
    let common = prepare_common(config).await?;
    let state = AppState {
        db_handles: common.db_handles,
        config: common.config,
        mail_sender: crate::services::mail_service::runtime_sender(common.runtime_config.clone()),
        runtime_config: common.runtime_config,
        cache: common.cache,
        metrics: common.metrics,
        background_task_dispatch_wakeup: AppState::new_background_task_dispatch_wakeup(),
    };

    tracing::info!(mode = "primary", "primary runtime startup complete");

    Ok(PreparedPrimaryRuntime { state })
}
