//! Shared value metadata and API/storage value conversion.
//!
//! Aster services store runtime configuration as strings, with selected keys
//! presented as JSON arrays for list-like values. This module keeps that
//! conversion consistent while leaving product-specific validation and default
//! generation to registries owned by each service.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::{ConfigCoreError, Result};

/// Supported system configuration value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValueType {
    /// A single-line string.
    String,
    /// A multi-line string.
    Multiline,
    /// A JSON array of strings.
    StringArray,
    /// One value selected from a known string enum.
    StringEnum,
    /// A JSON array of values selected from a known string enum.
    StringEnumSet,
    /// A numeric value stored as a string.
    Number,
    /// A boolean value stored as a string.
    Boolean,
}

impl ConfigValueType {
    /// Returns the canonical snake_case storage name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Multiline => "multiline",
            Self::StringArray => "string_array",
            Self::StringEnum => "string_enum",
            Self::StringEnumSet => "string_enum_set",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }

    /// Parses a canonical snake_case storage name.
    pub fn from_str_name(value: &str) -> Option<Self> {
        match value {
            "string" => Some(Self::String),
            "multiline" => Some(Self::Multiline),
            "string_array" => Some(Self::StringArray),
            "string_enum" => Some(Self::StringEnum),
            "string_enum_set" => Some(Self::StringEnumSet),
            "number" => Some(Self::Number),
            "boolean" => Some(Self::Boolean),
            _ => None,
        }
    }

    /// Returns whether this type stores a multi-line string.
    pub const fn is_multiline(self) -> bool {
        matches!(self, Self::Multiline)
    }

    /// Returns whether this type stores a JSON string array.
    pub const fn is_string_array(self) -> bool {
        matches!(self, Self::StringArray)
    }

    /// Returns whether this type stores a JSON string enum set.
    pub const fn is_string_enum_set(self) -> bool {
        matches!(self, Self::StringEnumSet)
    }

    /// Returns whether this type stores list-like strings.
    pub const fn is_string_list(self) -> bool {
        matches!(self, Self::StringArray | Self::StringEnumSet)
    }
}

impl fmt::Display for ConfigValueType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Origin of a stored configuration value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    /// Value is defined by the product registry.
    #[default]
    System,
    /// Value is user-defined and not backed by a product registry entry.
    Custom,
}

impl ConfigSource {
    /// Returns the canonical snake_case storage name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Custom => "custom",
        }
    }

    /// Parses a canonical snake_case storage name.
    pub fn from_str_name(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Consumer visibility for a stored configuration value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigVisibility {
    /// Only backend code and privileged APIs may see the value.
    #[default]
    Private,
    /// Anonymous clients may see the value.
    Public,
    /// Authenticated clients may see the value.
    Authenticated,
}

impl ConfigVisibility {
    /// Returns the canonical snake_case storage name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
            Self::Authenticated => "authenticated",
        }
    }

    /// Parses a canonical snake_case storage name.
    pub fn from_str_name(value: &str) -> Option<Self> {
        match value {
            "private" => Some(Self::Private),
            "public" => Some(Self::Public),
            "authenticated" => Some(Self::Authenticated),
            _ => None,
        }
    }

    /// Returns whether anonymous clients may see this value.
    pub const fn visible_to_public(self) -> bool {
        matches!(self, Self::Public)
    }

    /// Returns whether authenticated clients may see this value.
    pub const fn visible_to_authenticated(self) -> bool {
        matches!(self, Self::Public | Self::Authenticated)
    }
}

impl fmt::Display for ConfigVisibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// API-facing configuration value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    /// Scalar string value.
    String(String),
    /// JSON string-list value.
    StringArray(Vec<String>),
}

impl ConfigValue {
    /// Converts a storage string into an API-facing value for `value_type`.
    pub fn from_storage(value_type: ConfigValueType, value: String) -> Result<Self> {
        if !value_type.is_string_list() {
            return Ok(Self::String(value));
        }

        let items = serde_json::from_str::<Vec<String>>(&value)?;
        Ok(Self::StringArray(items))
    }

    /// Returns whether this value is logically empty.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::String(value) => value.trim().is_empty(),
            Self::StringArray(values) => values.is_empty(),
        }
    }

    /// Converts an API-facing value into a storage string for `value_type`.
    pub fn to_storage_for_type(&self, value_type: ConfigValueType) -> Result<String> {
        match (value_type, self) {
            (
                ConfigValueType::StringArray | ConfigValueType::StringEnumSet,
                Self::StringArray(values),
            ) => serde_json::to_string(values).map_err(Into::into),
            (ConfigValueType::StringArray | ConfigValueType::StringEnumSet, Self::String(_)) => {
                Err(ConfigCoreError::invalid_value(format!(
                    "{} config value must be a JSON array",
                    value_type.as_str()
                )))
            }
            (_, Self::String(value)) => Ok(value.clone()),
            (_, Self::StringArray(_)) => Err(ConfigCoreError::invalid_value(
                "string array values are only supported for string_array and string_enum_set config keys",
            )),
        }
    }

    /// Converts the value into an audit-friendly string.
    pub fn to_audit_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::StringArray(values) => serde_json::to_string(values)
                .unwrap_or_else(|_| "<invalid string list value>".to_string()),
        }
    }
}

impl From<&str> for ConfigValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<&String> for ConfigValue {
    fn from(value: &String) -> Self {
        Self::String(value.clone())
    }
}

impl From<String> for ConfigValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<Vec<String>> for ConfigValue {
    fn from(value: Vec<String>) -> Self {
        Self::StringArray(value)
    }
}

/// Validates a storage string against a declared value type.
///
/// This performs only product-neutral structural checks. Domain-specific rules,
/// such as enum membership or cross-field constraints, belong to registry
/// normalizers and dependency validators.
pub fn validate_storage_value(value_type: ConfigValueType, value: &str) -> Result<()> {
    let trimmed = value.trim();
    match value_type {
        ConfigValueType::Boolean => {
            if trimmed != "true" && trimmed != "false" {
                return Err(ConfigCoreError::invalid_value(
                    "boolean config must be 'true' or 'false'",
                ));
            }
        }
        ConfigValueType::Number => {
            if trimmed.parse::<f64>().is_err() {
                return Err(ConfigCoreError::invalid_value(
                    "number config must be a valid number",
                ));
            }
        }
        ConfigValueType::StringArray | ConfigValueType::StringEnumSet => {
            serde_json::from_str::<Vec<String>>(trimmed).map_err(|error| {
                ConfigCoreError::invalid_value(format!(
                    "{} config must be a JSON array of strings: {error}",
                    value_type.as_str()
                ))
            })?;
        }
        ConfigValueType::String | ConfigValueType::StringEnum | ConfigValueType::Multiline => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigSource, ConfigValue, ConfigValueType, ConfigVisibility, validate_storage_value,
    };

    #[test]
    fn value_type_round_trips_storage_names() {
        let cases = [
            (ConfigValueType::String, "string"),
            (ConfigValueType::Multiline, "multiline"),
            (ConfigValueType::StringArray, "string_array"),
            (ConfigValueType::StringEnum, "string_enum"),
            (ConfigValueType::StringEnumSet, "string_enum_set"),
            (ConfigValueType::Number, "number"),
            (ConfigValueType::Boolean, "boolean"),
        ];

        for (value_type, name) in cases {
            assert_eq!(value_type.as_str(), name);
            assert_eq!(value_type.to_string(), name);
            assert_eq!(ConfigValueType::from_str_name(name), Some(value_type));
        }
        assert_eq!(ConfigValueType::from_str_name("unknown"), None);
    }

    #[test]
    fn source_and_visibility_round_trip_storage_names() {
        assert_eq!(
            ConfigSource::from_str_name("system"),
            Some(ConfigSource::System)
        );
        assert_eq!(
            ConfigSource::from_str_name("custom"),
            Some(ConfigSource::Custom)
        );
        assert_eq!(ConfigSource::from_str_name("other"), None);

        assert_eq!(
            ConfigVisibility::from_str_name("private"),
            Some(ConfigVisibility::Private)
        );
        assert_eq!(
            ConfigVisibility::from_str_name("public"),
            Some(ConfigVisibility::Public)
        );
        assert_eq!(
            ConfigVisibility::from_str_name("authenticated"),
            Some(ConfigVisibility::Authenticated)
        );
        assert_eq!(ConfigVisibility::from_str_name("other"), None);
        assert!(ConfigVisibility::Public.visible_to_public());
        assert!(ConfigVisibility::Authenticated.visible_to_authenticated());
    }

    #[test]
    fn config_value_converts_storage_arrays_and_scalars() {
        assert_eq!(
            ConfigValue::from_storage(ConfigValueType::String, "hello".to_string()).unwrap(),
            ConfigValue::String("hello".to_string())
        );
        assert_eq!(
            ConfigValue::from_storage(ConfigValueType::StringArray, r#"["a","b"]"#.to_string(),)
                .unwrap(),
            ConfigValue::StringArray(vec!["a".to_string(), "b".to_string()])
        );

        let array = ConfigValue::StringArray(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            array
                .to_storage_for_type(ConfigValueType::StringEnumSet)
                .unwrap(),
            r#"["a","b"]"#
        );
        assert!(array.to_storage_for_type(ConfigValueType::String).is_err());
    }

    #[test]
    fn storage_value_validation_enforces_structural_types() {
        assert!(validate_storage_value(ConfigValueType::Boolean, "true").is_ok());
        assert!(validate_storage_value(ConfigValueType::Boolean, "yes").is_err());

        assert!(validate_storage_value(ConfigValueType::Number, "1.5").is_ok());
        assert!(validate_storage_value(ConfigValueType::Number, "abc").is_err());

        assert!(validate_storage_value(ConfigValueType::StringArray, r#"["a"]"#).is_ok());
        assert!(validate_storage_value(ConfigValueType::StringArray, r#""a""#).is_err());
        assert!(validate_storage_value(ConfigValueType::StringEnumSet, r#"["a"]"#).is_ok());
        assert!(validate_storage_value(ConfigValueType::StringEnumSet, r#""a""#).is_err());

        assert!(validate_storage_value(ConfigValueType::String, "anything").is_ok());
        assert!(validate_storage_value(ConfigValueType::StringEnum, "anything").is_ok());
        assert!(validate_storage_value(ConfigValueType::Multiline, "line\nline").is_ok());
    }
}
