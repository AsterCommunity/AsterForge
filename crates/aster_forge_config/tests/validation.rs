//! Compile-time and runtime checks for the configuration validation contract.

use aster_forge_config::{ConfigCoreError, ConfigSections, ValidatableConfig, ValidateConfig};

#[derive(Debug)]
struct ValidSection;

impl ValidatableConfig for ValidSection {
    fn validate(&self) -> aster_forge_config::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct InvalidSection;

impl ValidatableConfig for InvalidSection {
    fn validate(&self) -> aster_forge_config::Result<()> {
        Err(ConfigCoreError::invalid_value("invalid section"))
    }
}

#[derive(Debug, ValidateConfig)]
struct TestConfig {
    valid: ValidSection,
    invalid: InvalidSection,
}

#[derive(Debug, ValidateConfig)]
struct GenericConfig<T> {
    section: T,
}

impl<T: ValidatableConfig> ValidatableConfig for GenericConfig<T> {
    fn validate(&self) -> aster_forge_config::Result<()> {
        Ok(())
    }
}

impl ValidatableConfig for TestConfig {
    fn validate(&self) -> aster_forge_config::Result<()> {
        let _ = &self.valid;
        Ok(())
    }
}

#[test]
fn derive_validates_every_named_section() {
    let config = TestConfig {
        valid: ValidSection,
        invalid: InvalidSection,
    };
    assert_eq!(
        config.validate_sections().unwrap_err().to_string(),
        "invalid section"
    );
}

#[test]
fn loader_runs_sections_then_root_validation() {
    let result = aster_forge_config::load_config(|| {
        Ok::<_, &'static str>(TestConfig {
            valid: ValidSection,
            invalid: InvalidSection,
        })
    });
    assert_eq!(result.unwrap_err().to_string(), "invalid section");
}

#[test]
fn derive_preserves_generic_parameters() {
    let config = GenericConfig {
        section: ValidSection,
    };
    assert!(config.validate_sections().is_ok());
}

#[test]
fn loader_propagates_root_rules_after_successful_sections() {
    #[derive(Debug, ValidateConfig)]
    struct Root {
        section: ValidSection,
    }
    impl ValidatableConfig for Root {
        fn validate(&self) -> aster_forge_config::Result<()> {
            Err(ConfigCoreError::invalid_value("cross-section rule"))
        }
    }
    let result = aster_forge_config::load_config(|| {
        Ok::<_, &'static str>(Root {
            section: ValidSection,
        })
    });
    assert_eq!(result.unwrap_err().to_string(), "cross-section rule");
}
