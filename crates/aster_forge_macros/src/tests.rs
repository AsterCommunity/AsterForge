//! Wire representation and macro hygiene boundary tests.

wire_enum! {
    /// Test-only product state.
    pub(crate) enum State {
        /// Ready state.
        Ready => "ready",
        /// Hyphenated wire value.
        InProgress => "in-progress",
    }
}

#[test]
fn known_values_roundtrip_as_strings() {
    for (wire, expected) in [("ready", State::Ready), ("in-progress", State::InProgress)] {
        assert_eq!(State::from(wire.to_owned()), expected);
        assert_eq!(std::string::String::from(expected.clone()), wire);
        assert_eq!(
            serde_json::from_value::<State>(serde_json::json!(wire)).unwrap(),
            expected
        );
        assert_eq!(
            serde_json::to_value(expected).unwrap(),
            serde_json::json!(wire)
        );
    }
}

#[test]
fn unknown_strings_remain_exact_and_case_sensitive() {
    for wire in [
        "",
        "READY",
        "future",
        " ready ",
        "🚀",
        "a\0b",
        "line\nbreak",
        "\"quoted\"",
    ] {
        let expected = State::Unknown(wire.to_owned());
        assert_eq!(State::from(wire.to_owned()), expected);
        let json = serde_json::to_string(wire).unwrap();
        let parsed: State = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
        assert_eq!(std::string::String::from(parsed), wire);
    }
    let long = "x".repeat(65_536);
    let state: State = serde_json::from_value(serde_json::json!(long)).unwrap();
    assert_eq!(std::string::String::from(state), long);
}

#[test]
fn non_string_json_is_rejected() {
    for wire in ["null", "true", "0", "[]", "{}", "{\"Unknown\":\"future\"}"] {
        assert!(serde_json::from_str::<State>(wire).is_err());
    }
}

mod shadowed {
    // Generated code must not capture caller-local String or From names.
    struct String;
    trait From {}
    impl From for String {}
    wire_enum! { enum Local { Known => "known" } }

    fn uses_shadowed_trait<T: From>() {}

    #[test]
    fn generated_paths_are_hygienic() {
        uses_shadowed_trait::<String>();
        let value = Local::from(std::string::String::from("known"));
        assert_eq!(
            serde_json::to_value(value).unwrap(),
            serde_json::json!("known")
        );
    }
}
