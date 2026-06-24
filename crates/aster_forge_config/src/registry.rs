//! Configuration definition registry.
//!
//! Product crates register their keys here as static metadata. The registry is
//! then shared by default initialization, validation, admin APIs, frontend
//! settings pages, and code-generated documentation without every subsystem
//! inventing its own copy of the same definition list.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{
    ConfigCoreError, ConfigSource, ConfigValue, ConfigValueType, ConfigVisibility, Result,
    StoredConfig, validate_storage_value,
};

fn empty_default_value() -> String {
    String::new()
}

/// Lookup used by configuration normalizers and dependency validators.
pub trait ConfigValueLookup {
    /// Returns the current storage string for `key`.
    fn get_config_value(&self, key: &str) -> Option<String>;
}

impl ConfigValueLookup for HashMap<String, String> {
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.get(key).cloned()
    }
}

impl ConfigValueLookup for BTreeMap<String, String> {
    fn get_config_value(&self, key: &str) -> Option<String> {
        self.get(key).cloned()
    }
}

/// Product-owned value normalizer.
pub type ConfigNormalizer =
    fn(lookup: &dyn ConfigValueLookup, key: &str, value: &str) -> Result<String>;

/// Product-owned cross-field validator.
pub type ConfigDependencyValidator =
    fn(lookup: &dyn ConfigValueLookup, key: &str, normalized_value: &str) -> Result<()>;

/// Product-owned metadata for one configuration key.
#[derive(Debug, Clone, Copy)]
pub struct ConfigDefinition {
    /// Stable storage key.
    pub key: &'static str,
    /// Frontend i18n key for the display label.
    pub label_i18n_key: &'static str,
    /// Frontend i18n key for the description.
    pub description_i18n_key: &'static str,
    /// Storage and API value kind.
    pub value_type: ConfigValueType,
    /// Function returning the default storage value.
    pub default_fn: fn() -> String,
    /// Optional product-owned value normalizer.
    pub normalize_fn: Option<ConfigNormalizer>,
    /// Optional product-owned cross-field validator.
    pub dependency_validator_fn: Option<ConfigDependencyValidator>,
    /// Whether changes require process restart before they can take effect.
    pub requires_restart: bool,
    /// Whether values should be redacted in presentation and audit output.
    pub is_sensitive: bool,
    /// Default consumer visibility for this system-defined value.
    pub visibility: ConfigVisibility,
    /// Product-defined category key.
    pub category: &'static str,
    /// Backend-facing description used when initializing storage rows.
    pub description: &'static str,
}

impl ConfigDefinition {
    /// Returns a baseline private system definition for struct update syntax.
    ///
    /// Product registries usually repeat the same neutral metadata for most
    /// entries: system-owned source, private visibility, no normalizer, and no
    /// dependency validator. This helper keeps static definition lists compact
    /// while still requiring each product to spell out the storage key, type,
    /// default, category, and descriptions that define its public contract.
    pub const fn private_system() -> Self {
        Self {
            key: "",
            label_i18n_key: "",
            description_i18n_key: "",
            value_type: ConfigValueType::String,
            default_fn: empty_default_value,
            normalize_fn: None,
            dependency_validator_fn: None,
            requires_restart: false,
            is_sensitive: false,
            visibility: ConfigVisibility::Private,
            category: "",
            description: "",
        }
    }
}

/// Seed row produced from a registry definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSeedRecord {
    /// Stable storage key.
    pub key: String,
    /// Normalized storage string.
    pub value: String,
    /// Storage and API value kind.
    pub value_type: ConfigValueType,
    /// Whether changes require restart before they can take effect.
    pub requires_restart: bool,
    /// Whether values should be redacted in presentation and audit output.
    pub is_sensitive: bool,
    /// Source of this seeded value.
    pub source: ConfigSource,
    /// Default consumer visibility.
    pub visibility: ConfigVisibility,
    /// Product-defined category.
    pub category: String,
    /// Backend-facing description.
    pub description: String,
}

/// Static registry of product configuration definitions.
#[derive(Debug)]
pub struct ConfigRegistry {
    definitions: &'static [ConfigDefinition],
}

impl ConfigRegistry {
    /// Creates a registry from a static definition slice.
    pub const fn new(definitions: &'static [ConfigDefinition]) -> Self {
        Self { definitions }
    }

    /// Returns all registered definitions.
    pub const fn definitions(&self) -> &'static [ConfigDefinition] {
        self.definitions
    }

    /// Returns the definition for `key`.
    pub fn get(&self, key: &str) -> Option<&'static ConfigDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.key == key)
    }

    /// Returns whether `key` is registered.
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Returns the definition for `key` or an unknown-key error.
    pub fn require(&self, key: &str) -> Result<&'static ConfigDefinition> {
        self.get(key)
            .ok_or_else(|| ConfigCoreError::UnknownKey(key.to_string()))
    }

    /// Validates that keys are non-empty and unique.
    pub fn validate_unique_keys(&self) -> Result<()> {
        let mut seen = BTreeSet::new();
        for definition in self.definitions {
            if definition.key.trim().is_empty() {
                return Err(ConfigCoreError::invalid_value(
                    "config definition key cannot be empty",
                ));
            }
            if !seen.insert(definition.key) {
                return Err(ConfigCoreError::invalid_value(format!(
                    "duplicate config definition key '{}'",
                    definition.key
                )));
            }
        }
        Ok(())
    }

    /// Validates that all categories belong to `allowed_categories`.
    pub fn validate_categories(&self, allowed_categories: &[&str]) -> Result<()> {
        for definition in self.definitions {
            if !allowed_categories.contains(&definition.category) {
                return Err(ConfigCoreError::invalid_value(format!(
                    "config key '{}' uses unknown category '{}'",
                    definition.key, definition.category
                )));
            }
        }
        Ok(())
    }

    /// Validates a storage string for a known key.
    pub fn validate_value(&self, key: &str, value: &str) -> Result<()> {
        let definition = self.require(key)?;
        validate_storage_value(definition.value_type, value)
    }

    /// Normalizes a storage string for a known key.
    ///
    /// The input is expected to already match the structural storage shape for
    /// the definition's value type, for example a JSON string array for
    /// `string_array`.
    pub fn normalize_value(
        &self,
        lookup: &dyn ConfigValueLookup,
        key: &str,
        value: &str,
    ) -> Result<String> {
        let definition = self.require(key)?;
        validate_storage_value(definition.value_type, value)?;

        let normalized = match definition.normalize_fn {
            Some(normalize) => normalize(lookup, key, value)?,
            None => value.to_string(),
        };
        validate_storage_value(definition.value_type, &normalized)?;

        if let Some(validate) = definition.dependency_validator_fn {
            validate(lookup, key, &normalized)?;
        }

        Ok(normalized)
    }

    /// Converts an API-facing value into normalized storage for a known key.
    pub fn value_to_normalized_storage(
        &self,
        lookup: &dyn ConfigValueLookup,
        key: &str,
        value: &ConfigValue,
    ) -> Result<String> {
        let definition = self.require(key)?;
        let storage = value.to_storage_for_type(definition.value_type)?;
        self.normalize_value(lookup, key, &storage)
    }

    /// Applies definition metadata to a system-owned stored row.
    pub fn apply_definition(&self, mut config: StoredConfig) -> StoredConfig {
        if config.source != ConfigSource::System {
            return config;
        }

        let Some(definition) = self.get(&config.key) else {
            return config;
        };

        config.value_type = definition.value_type;
        config.requires_restart = definition.requires_restart;
        config.is_sensitive = definition.is_sensitive;
        config.visibility = definition.visibility;
        config.category = definition.category.to_string();
        config.description = definition.description.to_string();
        config
    }

    /// Builds default seed rows from the registry.
    ///
    /// Defaults are normalized in registry order. If a normalizer or
    /// dependency validator consults another config key, that dependency should
    /// appear earlier in the registry.
    pub fn default_seed_records(&self) -> Result<Vec<ConfigSeedRecord>> {
        let mut lookup = BTreeMap::<String, String>::new();
        let mut rows = Vec::with_capacity(self.definitions.len());

        for definition in self.definitions {
            let raw = (definition.default_fn)();
            let normalized = self.normalize_value(&lookup, definition.key, &raw)?;
            lookup.insert(definition.key.to_string(), normalized.clone());
            rows.push(ConfigSeedRecord {
                key: definition.key.to_string(),
                value: normalized,
                value_type: definition.value_type,
                requires_restart: definition.requires_restart,
                is_sensitive: definition.is_sensitive,
                source: ConfigSource::System,
                visibility: definition.visibility,
                category: definition.category.to_string(),
                description: definition.description.to_string(),
            });
        }

        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        ConfigDefinition, ConfigRegistry, ConfigSeedRecord, ConfigValueLookup, ConfigValueType,
    };
    use crate::{ConfigSource, ConfigValue, ConfigVisibility, StoredConfig};

    fn default_value() -> String {
        "default".to_string()
    }

    fn default_toggle() -> String {
        "true".to_string()
    }

    fn trim_value(
        _lookup: &dyn ConfigValueLookup,
        _key: &str,
        value: &str,
    ) -> crate::Result<String> {
        Ok(value.trim().to_string())
    }

    fn require_enabled(
        lookup: &dyn ConfigValueLookup,
        _key: &str,
        _value: &str,
    ) -> crate::Result<()> {
        match lookup.get_config_value("enabled").as_deref() {
            Some("true") => Ok(()),
            Some(_) => Err(crate::ConfigCoreError::invalid_value(
                "feature requires enabled=true",
            )),
            None => Err(crate::ConfigCoreError::invalid_value(
                "feature requires enabled to be present",
            )),
        }
    }

    const PRIMARY: ConfigDefinition = ConfigDefinition {
        key: "primary",
        label_i18n_key: "primary_label",
        description_i18n_key: "primary_desc",
        value_type: ConfigValueType::String,
        default_fn: default_value,
        normalize_fn: Some(trim_value),
        dependency_validator_fn: None,
        requires_restart: false,
        is_sensitive: false,
        visibility: ConfigVisibility::Private,
        category: "general",
        description: "primary setting",
    };

    const ENABLED: ConfigDefinition = ConfigDefinition {
        key: "enabled",
        label_i18n_key: "enabled_label",
        description_i18n_key: "enabled_desc",
        value_type: ConfigValueType::Boolean,
        default_fn: default_toggle,
        normalize_fn: None,
        dependency_validator_fn: None,
        requires_restart: false,
        is_sensitive: false,
        visibility: ConfigVisibility::Private,
        category: "general",
        description: "enabled flag",
    };

    const DEPENDENT: ConfigDefinition = ConfigDefinition {
        key: "dependent",
        label_i18n_key: "dependent_label",
        description_i18n_key: "dependent_desc",
        value_type: ConfigValueType::String,
        default_fn: default_value,
        normalize_fn: Some(trim_value),
        dependency_validator_fn: Some(require_enabled),
        requires_restart: true,
        is_sensitive: true,
        visibility: ConfigVisibility::Authenticated,
        category: "general",
        description: "dependent setting",
    };

    const DUPLICATE: ConfigDefinition = ConfigDefinition {
        key: "primary",
        ..PRIMARY
    };

    #[test]
    fn registry_finds_definitions_by_key() {
        let registry = ConfigRegistry::new(&[PRIMARY]);

        let definition = registry.require("primary").unwrap();

        assert_eq!(definition.key, "primary");
        assert!(registry.contains_key("primary"));
        assert!(!registry.contains_key("missing"));
    }

    #[test]
    fn registry_rejects_duplicate_keys_and_unknown_categories() {
        let duplicate_registry = ConfigRegistry::new(&[PRIMARY, DUPLICATE]);
        assert!(duplicate_registry.validate_unique_keys().is_err());

        let category_registry = ConfigRegistry::new(&[PRIMARY]);
        assert!(category_registry.validate_categories(&["general"]).is_ok());
        assert!(category_registry.validate_categories(&["other"]).is_err());
    }

    #[test]
    fn registry_normalizes_and_validates_known_values() {
        let registry = ConfigRegistry::new(&[ENABLED, DEPENDENT]);
        let lookup = HashMap::from([("enabled".to_string(), "true".to_string())]);

        assert_eq!(
            registry
                .normalize_value(&lookup, "dependent", "  demo  ")
                .unwrap(),
            "demo"
        );
        assert!(registry.normalize_value(&lookup, "enabled", "yes").is_err());
    }

    #[test]
    fn registry_dependency_validation_uses_lookup() {
        let registry = ConfigRegistry::new(&[ENABLED, DEPENDENT]);
        let failing_lookup = HashMap::from([("enabled".to_string(), "false".to_string())]);

        assert!(
            registry
                .normalize_value(&failing_lookup, "dependent", "value")
                .is_err()
        );
    }

    #[test]
    fn registry_converts_api_values_into_normalized_storage() {
        let registry = ConfigRegistry::new(&[PRIMARY]);

        assert_eq!(
            registry
                .value_to_normalized_storage(
                    &HashMap::new(),
                    "primary",
                    &ConfigValue::from("  x  ")
                )
                .unwrap(),
            "x"
        );
    }

    #[test]
    fn registry_applies_definition_metadata_to_system_rows() {
        let registry = ConfigRegistry::new(&[DEPENDENT]);
        let row = StoredConfig {
            id: 7,
            key: "dependent".to_string(),
            value: "value".to_string(),
            value_type: ConfigValueType::String,
            requires_restart: false,
            is_sensitive: false,
            source: ConfigSource::System,
            visibility: ConfigVisibility::Private,
            category: String::new(),
            description: String::new(),
        };

        let applied = registry.apply_definition(row);
        assert_eq!(applied.value_type, ConfigValueType::String);
        assert!(applied.requires_restart);
        assert!(applied.is_sensitive);
        assert_eq!(applied.visibility, ConfigVisibility::Authenticated);
        assert_eq!(applied.category, "general");
        assert_eq!(applied.description, "dependent setting");
    }

    #[test]
    fn registry_builds_normalized_default_seed_records() {
        let registry = ConfigRegistry::new(&[ENABLED, DEPENDENT]);

        assert_eq!(
            registry.default_seed_records().unwrap(),
            vec![
                ConfigSeedRecord {
                    key: "enabled".to_string(),
                    value: "true".to_string(),
                    value_type: ConfigValueType::Boolean,
                    requires_restart: false,
                    is_sensitive: false,
                    source: ConfigSource::System,
                    visibility: ConfigVisibility::Private,
                    category: "general".to_string(),
                    description: "enabled flag".to_string(),
                },
                ConfigSeedRecord {
                    key: "dependent".to_string(),
                    value: "default".to_string(),
                    value_type: ConfigValueType::String,
                    requires_restart: true,
                    is_sensitive: true,
                    source: ConfigSource::System,
                    visibility: ConfigVisibility::Authenticated,
                    category: "general".to_string(),
                    description: "dependent setting".to_string(),
                },
            ]
        );
    }
}
