//! 配置模块导出与全局入口。

pub mod audit;
pub mod auth_runtime;
pub mod bool_like;
pub mod branding;
pub mod cors;
pub mod definitions;
mod loader;
pub mod local_email_policy;
pub mod mail;
pub mod node_mode;
pub mod operations;
mod runtime_config;
mod schema;
pub mod site_url;
pub mod system_config;

pub use runtime_config::RuntimeConfig;
pub use schema::{
    AuthConfig, CacheConfig, Config, DatabaseConfig, LoggingConfig, NetworkTrustConfig,
    RateLimitConfig, RateLimitTier, ServerConfig, ServerFollowerConfig,
};

use std::sync::Arc;
use std::sync::OnceLock;

static CONFIG: OnceLock<Arc<Config>> = OnceLock::new();

pub fn ensure_default_config_for_current_dir(
    default: &Config,
) -> crate::errors::Result<std::path::PathBuf> {
    loader::ensure_default_config_for_current_dir(default)
}

pub fn init_config() -> crate::errors::Result<Arc<Config>> {
    let cfg = loader::load()?;
    let cfg = CONFIG.get_or_init(|| Arc::new(cfg)).clone();
    Ok(cfg)
}

pub fn get_config() -> Arc<Config> {
    CONFIG
        .get()
        .expect("Config not initialized. Call init_config() first.")
        .clone()
}

/// 尝试获取配置，未初始化时返回 None。
pub fn try_get_config() -> Option<Arc<Config>> {
    CONFIG.get().cloned()
}

/// 测试环境用：手动设置全局配置（OnceLock 只接受第一次调用）
pub fn set_config_for_test(config: Arc<Config>) -> Result<(), Arc<Config>> {
    CONFIG.set(config)
}
