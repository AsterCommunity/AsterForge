//! Public registry write contracts and short-circuit boundaries.

use std::cell::RefCell;

use aster_forge_config::{
    ConfigCoreError, ConfigDefinition, ConfigRegistry, ConfigValue, ConfigValueLookup,
    ConfigValueType, Result, normalize_bounded_u64_config_value,
};

fn normalize_ttl(lookup: &dyn ConfigValueLookup, key: &str, value: &str) -> Result<String> {
    lookup.get_config_value("normalizer_called");
    if value == "13" {
        // Simulate a faulty product normalizer to exercise output structure validation.
        return Ok("not-a-number".to_string());
    }
    normalize_bounded_u64_config_value(key, value, 1, 60)
}

fn validate_dependency(lookup: &dyn ConfigValueLookup, _key: &str, value: &str) -> Result<()> {
    lookup.get_config_value("dependency_called");
    assert!(matches!(value, "1" | "5" | "60"));
    if lookup.get_config_value("enabled").as_deref() != Some("true") {
        return Err(ConfigCoreError::invalid_value("requires enabled=true"));
    }
    Ok(())
}

const TTL: ConfigDefinition = ConfigDefinition {
    key: "ttl",
    value_type: ConfigValueType::Number,
    normalize_fn: Some(normalize_ttl),
    dependency_validator_fn: Some(validate_dependency),
    ..ConfigDefinition::private_system()
};

static REGISTRY: ConfigRegistry = ConfigRegistry::new(&[TTL]);

#[test]
fn structure_validation_does_not_enforce_domain_rules() {
    for value in ["0", "61", "13", "1.5", "-1"] {
        REGISTRY.validate_value_structure("ttl", value).unwrap();
    }
    for value in ["", "text", "NaN", "inf", "-inf", "1e999"] {
        assert!(REGISTRY.validate_value_structure("ttl", value).is_err());
    }
    assert!(matches!(
        REGISTRY.validate_value_structure("unknown", "5"),
        Err(ConfigCoreError::UnknownKey(key)) if key == "unknown"
    ));
}

#[derive(Clone, Copy, Debug)]
enum EntryPoint {
    LogicalString,
    KnownApiValue,
    RegisteredOrCustomApiValue,
}

fn normalize(entry: EntryPoint, lookup: &dyn ConfigValueLookup, value: &str) -> Result<String> {
    match entry {
        EntryPoint::LogicalString => REGISTRY.normalize_value(lookup, "ttl", value),
        EntryPoint::KnownApiValue => {
            REGISTRY.value_to_normalized_storage(lookup, "ttl", &ConfigValue::from(value))
        }
        EntryPoint::RegisteredOrCustomApiValue => {
            REGISTRY.value_to_storage_for_key(lookup, "ttl", &ConfigValue::from(value))
        }
    }
}

#[test]
fn every_full_validation_entry_obeys_stage_order_and_short_circuits() {
    for entry in [
        EntryPoint::LogicalString,
        EntryPoint::KnownApiValue,
        EntryPoint::RegisteredOrCustomApiValue,
    ] {
        for (input, enabled, expected_stages, expected_value) in [
            ("text", true, vec![], None),
            ("NaN", true, vec![], None),
            ("61", true, vec!["normalizer_called"], None),
            ("13", true, vec!["normalizer_called"], None),
            (
                "005",
                false,
                vec!["normalizer_called", "dependency_called", "enabled"],
                None,
            ),
            (
                "005",
                true,
                vec!["normalizer_called", "dependency_called", "enabled"],
                Some("5"),
            ),
        ] {
            let stages = RefCell::new(Vec::new());
            let lookup = |key: &str| {
                stages.borrow_mut().push(key.to_string());
                (key == "enabled" && enabled).then(|| "true".to_string())
            };
            let result = normalize(entry, &lookup, input);
            match expected_value {
                Some(expected) => assert_eq!(result.unwrap(), expected, "{entry:?}: {input}"),
                None => assert!(result.is_err(), "{entry:?}: {input}"),
            }
            assert_eq!(*stages.borrow(), expected_stages, "{entry:?}: {input}");
        }
    }
}

#[test]
fn domain_normalizer_enforces_closed_integer_bounds() {
    let lookup = |key: &str| (key == "enabled").then(|| "true".to_string());
    for (input, expected) in [("1", "1"), ("60", "60"), (" 005 ", "5")] {
        assert_eq!(
            REGISTRY.normalize_value(&lookup, "ttl", input).unwrap(),
            expected
        );
    }
    for input in [
        "0",
        "61",
        "-1",
        "1.5",
        "1e1",
        "18446744073709551616",
        "",
        "NaN",
        "inf",
        "-inf",
    ] {
        assert!(
            REGISTRY.normalize_value(&lookup, "ttl", input).is_err(),
            "{input}"
        );
    }
}

#[test]
fn missing_dependency_is_rejected() {
    let lookup = |_: &str| None;
    let error = REGISTRY.normalize_value(&lookup, "ttl", "5").unwrap_err();
    assert!(
        matches!(error, ConfigCoreError::InvalidValue(message) if message == "requires enabled=true")
    );
}

#[test]
fn unknown_key_and_wrong_api_shape_do_not_invoke_callbacks() {
    let lookup = |_: &str| -> Option<String> { panic!("callbacks must not run") };
    for result in [
        REGISTRY.normalize_value(&lookup, "unknown", "5"),
        REGISTRY.value_to_normalized_storage(&lookup, "unknown", &ConfigValue::from("5")),
    ] {
        assert!(matches!(result, Err(ConfigCoreError::UnknownKey(key)) if key == "unknown"));
    }
    let array = ConfigValue::StringArray(vec!["5".to_string()]);
    assert!(
        REGISTRY
            .value_to_normalized_storage(&lookup, "ttl", &array)
            .is_err()
    );
    assert!(
        REGISTRY
            .value_to_storage_for_key(&lookup, "ttl", &array)
            .is_err()
    );
}

#[test]
fn custom_scalar_path_preserves_values_and_leaves_domain_rules_to_product() {
    let lookup = |_: &str| -> Option<String> { panic!("custom keys have no registry callbacks") };
    for input in [
        "",
        "  custom value  ",
        "NaN",
        "[not JSON]",
        "secret:v1:opaque",
    ] {
        assert_eq!(
            REGISTRY
                .value_to_storage_for_key(&lookup, "custom.key", &ConfigValue::from(input))
                .unwrap(),
            input
        );
    }
    assert!(
        REGISTRY
            .value_to_storage_for_key(&lookup, "custom.key", &ConfigValue::StringArray(vec![]))
            .is_err()
    );
}

#[test]
fn definitions_without_callbacks_still_validate_structure_and_preserve_values() {
    static PLAIN: ConfigRegistry = ConfigRegistry::new(&[ConfigDefinition {
        normalize_fn: None,
        dependency_validator_fn: None,
        ..TTL
    }]);
    let lookup = |_: &str| -> Option<String> { panic!("no callbacks are registered") };
    assert_eq!(
        PLAIN.normalize_value(&lookup, "ttl", " 61 ").unwrap(),
        " 61 "
    );
    assert!(
        PLAIN
            .normalize_value(&lookup, "ttl", "not-a-number")
            .is_err()
    );
}

#[test]
fn optional_callbacks_run_independently() {
    static NORMALIZER_ONLY: ConfigRegistry = ConfigRegistry::new(&[ConfigDefinition {
        dependency_validator_fn: None,
        ..TTL
    }]);
    static DEPENDENCY_ONLY: ConfigRegistry = ConfigRegistry::new(&[ConfigDefinition {
        normalize_fn: None,
        ..TTL
    }]);
    let stages = RefCell::new(Vec::new());
    let lookup = |key: &str| {
        stages.borrow_mut().push(key.to_string());
        (key == "enabled").then(|| "true".to_string())
    };
    assert_eq!(
        NORMALIZER_ONLY
            .normalize_value(&lookup, "ttl", "005")
            .unwrap(),
        "5"
    );
    assert_eq!(*stages.borrow(), ["normalizer_called"]);
    stages.borrow_mut().clear();
    assert_eq!(
        DEPENDENCY_ONLY
            .normalize_value(&lookup, "ttl", "5")
            .unwrap(),
        "5"
    );
    assert_eq!(*stages.borrow(), ["dependency_called", "enabled"]);
}

#[test]
fn array_and_boolean_structure_boundaries_match_registered_types() {
    static REGISTRY: ConfigRegistry = ConfigRegistry::new(&[
        ConfigDefinition {
            key: "array",
            value_type: ConfigValueType::StringArray,
            ..ConfigDefinition::private_system()
        },
        ConfigDefinition {
            key: "boolean",
            value_type: ConfigValueType::Boolean,
            ..ConfigDefinition::private_system()
        },
    ]);
    for value in ["[]", r#"["", "https://example.com"]"#] {
        REGISTRY.validate_value_structure("array", value).unwrap();
    }
    for value in ["", "null", "{}", "[1]", "[true]", "[null]", "[\"x\",]"] {
        assert!(
            REGISTRY.validate_value_structure("array", value).is_err(),
            "{value}"
        );
    }
    for value in ["true", "false"] {
        REGISTRY.validate_value_structure("boolean", value).unwrap();
    }
    for value in ["", "yes", "1", "null"] {
        assert!(
            REGISTRY.validate_value_structure("boolean", value).is_err(),
            "{value}"
        );
    }
}
