//! Shared value metadata and API/storage value conversion.
//!
//! Aster services store runtime configuration as strings, with selected keys
//! presented as JSON arrays for list-like values. This module keeps that
//! conversion consistent while leaving product-specific validation and default
//! generation to registries owned by each service.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::{ConfigCoreError, Result};
#[cfg(feature = "sea-orm")]
use sea_orm::entity::prelude::*;

/// Supported system configuration value types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
#[cfg_attr(feature = "sea-orm", derive(EnumIter, DeriveActiveEnum))]
#[cfg_attr(
    feature = "sea-orm",
    sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigValueType {
    /// A single-line string.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "string"))]
    String,
    /// A multi-line string.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "multiline"))]
    Multiline,
    /// A JSON array of strings.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "string_array"))]
    StringArray,
    /// One value selected from a known string enum.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "string_enum"))]
    StringEnum,
    /// A JSON array of values selected from a known string enum.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "string_enum_set"))]
    StringEnumSet,
    /// A numeric value stored as a string.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "number"))]
    Number,
    /// A boolean value stored as a string.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "boolean"))]
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
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
#[cfg_attr(feature = "sea-orm", derive(EnumIter, DeriveActiveEnum))]
#[cfg_attr(
    feature = "sea-orm",
    sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    /// Value is defined by the product registry.
    #[default]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "system"))]
    System,
    /// Value is user-defined and not backed by a product registry entry.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "custom"))]
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
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
#[cfg_attr(feature = "sea-orm", derive(EnumIter, DeriveActiveEnum))]
#[cfg_attr(
    feature = "sea-orm",
    sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")
)]
#[serde(rename_all = "snake_case")]
pub enum ConfigVisibility {
    /// Only backend code and privileged APIs may see the value.
    #[default]
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "private"))]
    Private,
    /// Anonymous clients may see the value.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "public"))]
    Public,
    /// Authenticated clients may see the value.
    #[cfg_attr(feature = "sea-orm", sea_orm(string_value = "authenticated"))]
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
#[cfg_attr(all(debug_assertions, feature = "openapi"), derive(utoipa::ToSchema))]
pub enum ConfigValue {
    /// Scalar string value.
    String(String),
    /// JSON string-list value.
    StringArray(Vec<String>),
}

impl ConfigValue {
    /// Redacted value used when presenting sensitive configuration through APIs or audit logs.
    pub const REDACTED: &'static str = "***REDACTED***";

    /// Converts a storage string into an API-facing value for `value_type`.
    pub fn from_storage(value_type: impl Into<ConfigValueType>, value: String) -> Result<Self> {
        let value_type = value_type.into();
        if !value_type.is_string_list() {
            return Ok(Self::String(value));
        }

        let items = serde_json::from_str::<Vec<String>>(&value)?;
        Ok(Self::StringArray(items))
    }

    /// Converts a storage string into an API-facing value and falls back to an empty value on
    /// malformed stored data.
    ///
    /// Products should still validate writes strictly. This helper is for read/presentation paths
    /// where a single bad database row should not break the entire admin config page.
    pub fn from_storage_lossy(
        value_type: impl Into<ConfigValueType>,
        value: String,
        on_invalid: impl FnOnce(&ConfigCoreError),
    ) -> Self {
        let value_type = value_type.into();
        match Self::from_storage(value_type, value) {
            Ok(value) => value,
            Err(error) => {
                on_invalid(&error);
                Self::empty_for_type(value_type)
            }
        }
    }

    /// Returns the canonical redacted API value.
    pub fn redacted() -> Self {
        Self::String(Self::REDACTED.to_string())
    }

    /// Returns the empty API value appropriate for a declared value type.
    pub fn empty_for_type(value_type: impl Into<ConfigValueType>) -> Self {
        if value_type.into().is_string_list() {
            Self::StringArray(Vec::new())
        } else {
            Self::String(String::new())
        }
    }

    /// Returns whether this value is logically empty.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::String(value) => value.trim().is_empty(),
            Self::StringArray(values) => values.is_empty(),
        }
    }

    /// Converts an API-facing value into a storage string for `value_type`.
    pub fn to_storage_for_type(&self, value_type: impl Into<ConfigValueType>) -> Result<String> {
        let value_type = value_type.into();
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

/// Builds a configuration value for API presentation.
///
/// Sensitive values are redacted and malformed historical storage values fall back to an empty
/// value for the declared type. Products should use this in API read/list paths instead of
/// repeating redaction and lossy parsing logic in each service.
pub fn present_config_value(
    value_type: impl Into<ConfigValueType>,
    value: String,
    is_sensitive: bool,
    on_invalid: impl FnOnce(&ConfigCoreError),
) -> ConfigValue {
    if is_sensitive {
        ConfigValue::redacted()
    } else {
        ConfigValue::from_storage_lossy(value_type, value, on_invalid)
    }
}

/// Builds an audit-safe string from a stored configuration value.
///
/// Sensitive values are always represented by [`ConfigValue::REDACTED`]. Non-sensitive values use
/// the same lossy read path as API presentation so audit recording does not fail because of one
/// malformed historical row.
pub fn config_value_audit_string(
    value_type: impl Into<ConfigValueType>,
    value: String,
    is_sensitive: bool,
    on_invalid: impl FnOnce(&ConfigCoreError),
) -> String {
    if is_sensitive {
        ConfigValue::REDACTED.to_string()
    } else {
        ConfigValue::from_storage_lossy(value_type, value, on_invalid).to_audit_string()
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
            // f64 parsing accepts NaN/inf literals; storing them would propagate
            // non-finite values into every reader's arithmetic.
            if !trimmed.parse::<f64>().is_ok_and(f64::is_finite) {
                return Err(ConfigCoreError::invalid_value(
                    "number config must be a valid finite number",
                ));
            }
        }
        ConfigValueType::StringArray | ConfigValueType::StringEnumSet => {
            parse_string_array_config_value(trimmed, value_type.as_str())?;
        }
        ConfigValueType::String | ConfigValueType::StringEnum | ConfigValueType::Multiline => {}
    }
    Ok(())
}

/// Parses a JSON array of strings stored in a configuration value.
///
/// Product crates can use this before applying domain-specific normalization
/// such as URL canonicalization, domain lower-casing, allow-list filtering, or
/// duplicate removal. The `key` is only used to produce a precise validation
/// error.
pub fn parse_string_array_config_value(value: &str, key: &str) -> Result<Vec<String>> {
    serde_json::from_str::<Vec<String>>(value.trim()).map_err(|error| {
        ConfigCoreError::invalid_value(format!("{key} must be a JSON array of strings: {error}"))
    })
}

/// Parses a single string enum value with legacy single-item array compatibility.
///
/// New `string_enum` config values should be stored as scalar strings. Some older Aster
/// deployments stored single-select values as a JSON array with exactly one string because the UI
/// previously treated them like enum sets. This helper keeps that migration-compatible shape in one
/// place while product crates still own the concrete enum and accepted values.
pub fn parse_single_string_enum_selection<T>(
    value: &str,
    key: &str,
    allowed_values: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<T> {
    let trimmed = value.trim();
    let selected = if trimmed.starts_with('[') {
        let items = serde_json::from_str::<Vec<String>>(trimmed).map_err(|error| {
            ConfigCoreError::invalid_value(format!(
                "{key} must be a string enum or a legacy JSON array with exactly one value: {error}",
            ))
        })?;
        let [item] = items.as_slice() else {
            return Err(ConfigCoreError::invalid_value(format!(
                "{key} must select exactly one value",
            )));
        };
        item.clone()
    } else {
        trimmed.to_string()
    };

    parse(&selected).ok_or_else(|| {
        ConfigCoreError::invalid_value(format!("{key} must be one of: {allowed_values}"))
    })
}

/// Parses a string enum set from a JSON array of strings.
///
/// Product crates still own the concrete enum, canonical names, allowed values,
/// and default set. Forge only handles the shared storage shape and duplicate
/// detection so every service reports consistent malformed enum-set config.
pub fn parse_string_enum_set_selection<T>(
    value: &str,
    key: &str,
    item_name: &str,
    parse: impl Fn(&str) -> Option<T>,
) -> Result<Vec<T>>
where
    T: Copy + Eq,
{
    let values = parse_string_array_config_value(value, key)?;
    let mut selected = Vec::with_capacity(values.len());

    for raw in values {
        let Some(item) = parse(&raw) else {
            return Err(ConfigCoreError::invalid_value(format!(
                "unknown {item_name} '{raw}' in {key}"
            )));
        };
        if selected.contains(&item) {
            return Err(ConfigCoreError::invalid_value(format!(
                "duplicate {item_name} '{raw}' in {key}"
            )));
        }
        selected.push(item);
    }

    Ok(selected)
}

/// Parses and normalizes a string enum set into authoritative order.
///
/// This is useful for `string_enum_set` config values whose storage order should
/// stay stable regardless of the order provided by an API request. The returned
/// values are the canonical storage strings from `display`.
pub fn normalize_string_enum_set_selection<T>(
    value: &str,
    key: &str,
    item_name: &str,
    authoritative_order: &[T],
    parse: impl Fn(&str) -> Option<T>,
    display: impl Fn(T) -> &'static str,
) -> Result<Vec<&'static str>>
where
    T: Copy + Eq,
{
    let selected = parse_string_enum_set_selection(value, key, item_name, parse)?;
    Ok(authoritative_order
        .iter()
        .copied()
        .filter(|item| selected.contains(item))
        .map(display)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        ConfigSource, ConfigValue, ConfigValueType, ConfigVisibility, config_value_audit_string,
        normalize_string_enum_set_selection, parse_single_string_enum_selection,
        parse_string_array_config_value, parse_string_enum_set_selection, present_config_value,
        validate_storage_value,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TestEnum {
        Fast,
        Balanced,
        Quality,
    }

    const TEST_ENUMS: &[TestEnum] = &[TestEnum::Fast, TestEnum::Balanced, TestEnum::Quality];

    fn parse_test_enum(value: &str) -> Option<TestEnum> {
        match value {
            "fast" => Some(TestEnum::Fast),
            "balanced" => Some(TestEnum::Balanced),
            "quality" => Some(TestEnum::Quality),
            _ => None,
        }
    }

    fn display_test_enum(value: TestEnum) -> &'static str {
        match value {
            TestEnum::Fast => "fast",
            TestEnum::Balanced => "balanced",
            TestEnum::Quality => "quality",
        }
    }

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
    fn config_value_presentation_redacts_and_falls_back_lossily() {
        let mut saw_error = false;
        let value = present_config_value(
            ConfigValueType::StringArray,
            "not json".to_string(),
            false,
            |_| saw_error = true,
        );
        assert!(saw_error);
        assert_eq!(value, ConfigValue::StringArray(Vec::new()));

        let value = present_config_value(
            ConfigValueType::StringArray,
            r#"["secret"]"#.to_string(),
            true,
            |_| unreachable!("redacted values should not parse storage"),
        );
        assert_eq!(
            value,
            ConfigValue::String(ConfigValue::REDACTED.to_string())
        );
    }

    #[test]
    fn config_value_audit_string_redacts_and_serializes_lossily() {
        let audit = config_value_audit_string(
            ConfigValueType::StringArray,
            r#"["b","a"]"#.to_string(),
            false,
            |_| unreachable!("valid storage should not report errors"),
        );
        assert_eq!(audit, r#"["b","a"]"#);

        let audit =
            config_value_audit_string(ConfigValueType::String, "secret".to_string(), true, |_| {
                unreachable!("redacted values should not parse storage")
            });
        assert_eq!(audit, ConfigValue::REDACTED);

        let mut saw_error = false;
        let audit = config_value_audit_string(
            ConfigValueType::StringArray,
            "not json".to_string(),
            false,
            |_| saw_error = true,
        );
        assert!(saw_error);
        assert_eq!(audit, "[]");
    }

    #[test]
    fn storage_value_validation_enforces_structural_types() {
        assert!(validate_storage_value(ConfigValueType::Boolean, "true").is_ok());
        assert!(validate_storage_value(ConfigValueType::Boolean, "yes").is_err());

        assert!(validate_storage_value(ConfigValueType::Number, "1.5").is_ok());
        assert!(validate_storage_value(ConfigValueType::Number, "abc").is_err());

        // f64 parsing accepts non-finite literals, but NaN/inf stored as config
        // would propagate into every reader's arithmetic.
        assert!(validate_storage_value(ConfigValueType::Number, "NaN").is_err());
        assert!(validate_storage_value(ConfigValueType::Number, "inf").is_err());
        assert!(validate_storage_value(ConfigValueType::Number, "-inf").is_err());
        assert!(validate_storage_value(ConfigValueType::Number, "1e308").is_ok());

        assert!(validate_storage_value(ConfigValueType::StringArray, r#"["a"]"#).is_ok());
        assert!(validate_storage_value(ConfigValueType::StringArray, r#""a""#).is_err());
        assert!(validate_storage_value(ConfigValueType::StringEnumSet, r#"["a"]"#).is_ok());
        assert!(validate_storage_value(ConfigValueType::StringEnumSet, r#""a""#).is_err());

        assert!(validate_storage_value(ConfigValueType::String, "anything").is_ok());
        assert!(validate_storage_value(ConfigValueType::StringEnum, "anything").is_ok());
        assert!(validate_storage_value(ConfigValueType::Multiline, "line\nline").is_ok());
    }

    #[test]
    fn single_string_enum_selection_accepts_scalar_and_legacy_single_array() {
        let parse = |value: &str| match value {
            "fast" | "quality" => Some(value.to_string()),
            _ => None,
        };

        assert_eq!(
            parse_single_string_enum_selection(
                " fast ",
                "preview_profile",
                "fast or quality",
                parse
            )
            .unwrap(),
            "fast"
        );
        assert_eq!(
            parse_single_string_enum_selection(
                r#"["quality"]"#,
                "preview_profile",
                "fast or quality",
                parse,
            )
            .unwrap(),
            "quality"
        );
    }

    #[test]
    fn single_string_enum_selection_rejects_invalid_legacy_arrays_and_values() {
        let parse = |value: &str| (value == "fast").then_some(value.to_string());

        assert!(
            parse_single_string_enum_selection(r#"[]"#, "preview_profile", "fast", parse).is_err()
        );
        assert!(
            parse_single_string_enum_selection(
                r#"["fast","quality"]"#,
                "preview_profile",
                "fast",
                parse,
            )
            .is_err()
        );
        assert!(
            parse_single_string_enum_selection(r#"["unknown"]"#, "preview_profile", "fast", parse,)
                .is_err()
        );
        assert!(
            parse_single_string_enum_selection("unknown", "preview_profile", "fast", parse)
                .is_err()
        );
    }

    #[test]
    fn string_array_config_value_parses_json_string_arrays() {
        assert_eq!(
            parse_string_array_config_value(r#"["a","b"]"#, "domains").unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(parse_string_array_config_value(r#""a""#, "domains").is_err());
        assert!(parse_string_array_config_value(r#"[1]"#, "domains").is_err());
    }

    #[test]
    fn string_enum_set_selection_rejects_unknown_and_duplicate_values() {
        assert_eq!(
            parse_string_enum_set_selection(
                r#"["quality","fast"]"#,
                "preview_profiles",
                "preview profile",
                parse_test_enum,
            )
            .unwrap(),
            vec![TestEnum::Quality, TestEnum::Fast]
        );
        assert!(
            parse_string_enum_set_selection(
                r#"["fast","unknown"]"#,
                "preview_profiles",
                "preview profile",
                parse_test_enum,
            )
            .is_err()
        );
        assert!(
            parse_string_enum_set_selection(
                r#"["fast","fast"]"#,
                "preview_profiles",
                "preview profile",
                parse_test_enum,
            )
            .is_err()
        );
    }

    #[test]
    fn string_enum_set_selection_normalizes_to_authoritative_order() {
        assert_eq!(
            normalize_string_enum_set_selection(
                r#"["quality","fast"]"#,
                "preview_profiles",
                "preview profile",
                TEST_ENUMS,
                parse_test_enum,
                display_test_enum,
            )
            .unwrap(),
            vec!["fast", "quality"]
        );
        assert_eq!(
            normalize_string_enum_set_selection(
                r#"[]"#,
                "preview_profiles",
                "preview profile",
                TEST_ENUMS,
                parse_test_enum,
                display_test_enum,
            )
            .unwrap(),
            Vec::<&'static str>::new()
        );
    }
}
