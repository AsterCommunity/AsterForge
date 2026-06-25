//! Shared runtime configuration primitives for Aster services.
//!
//! This crate owns product-neutral configuration mechanics: typed configuration
//! definitions, registry construction, storage value conversion, in-process
//! runtime snapshots, reload diffing, and cross-process reload notifications.
//! Product crates still own their concrete database entities, repositories,
//! localized labels, config keys, domain-specific normalizers, and any derived
//! runtime state that is built from configuration values.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

mod error;
mod notification;
mod registry;
mod runtime;
mod value;

pub use error::{ConfigCoreError, Result};
pub use notification::{
    ConfigChangeEvent, ConfigChangeNotifier, ConfigNotification, ConfigNotificationSource,
    ConfigReloadMessage, InMemoryConfigNotifier,
};
#[cfg(feature = "redis")]
pub use notification::{RedisConfigChangeNotifier, RedisConfigReloadListener};
pub use registry::{
    ConfigDefinition, ConfigDependencyValidator, ConfigNormalizer, ConfigRegistry,
    ConfigSeedRecord, ConfigValueLookup,
};
pub use runtime::{
    AsyncConfigSnapshot, AsyncConfigStore, AsyncRuntimeConfig, RuntimeConfigChange,
    RuntimeConfigRecord, StoredConfig, SyncConfigSnapshot, SyncRuntimeConfig,
    normalize_positive_u64_config_value, parse_bool_like_value, parse_bounded_u64,
    parse_non_negative_u64, parse_positive_i32, parse_positive_u64, read_bool, read_bounded_u64,
    read_non_negative_u64, read_positive_i32, read_positive_u64, read_positive_usize,
};
pub use value::{
    ConfigSource, ConfigValue, ConfigValueType, ConfigVisibility,
    parse_single_string_enum_selection, parse_string_array_config_value, validate_storage_value,
};

/// Builds a static [`ConfigRegistry`] from a list of [`ConfigDefinition`] items.
///
/// Product crates normally wrap this macro in their own module that names
/// product-specific keys and default functions. Keeping registration declarative
/// makes it easier for services to hand the same registry to default
/// initialization, validation, OpenAPI presentation, and admin UI metadata.
#[macro_export]
macro_rules! define_config_registry {
    ($vis:vis static $name:ident = [$($definition:expr),* $(,)?];) => {
        $vis static $name: $crate::ConfigRegistry = $crate::ConfigRegistry::new(&[
            $($definition),*
        ]);
    };
}
