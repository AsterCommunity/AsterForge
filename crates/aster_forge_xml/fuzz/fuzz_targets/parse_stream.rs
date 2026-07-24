#![no_main]

use aster_forge_xml::{
    BorrowedDocument, ParseOptions, XmlSafetyPolicy, XmlStreamEvent, XmlStreamReader,
    validate_xml_input,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let max_input_bytes = input.len().max(1);
    let policy = XmlSafetyPolicy {
        max_input_bytes,
        max_depth: 128,
        max_elements: 65_536,
        max_attributes_per_element: 1_024,
        max_text_bytes: max_input_bytes,
        max_events: 262_144,
        reject_doctype: true,
    };
    let validation = validate_xml_input(input, policy);
    let options = ParseOptions::new().safety_policy(policy);
    let document = BorrowedDocument::parse_with_options(input, &options);
    if let Ok(document) = document {
        assert!(validation.is_ok());
        let mut original = Vec::new();
        document
            .write_original(&mut original)
            .expect("Vec writes do not fail");
        assert_eq!(original, input);
    }

    let mut reader = XmlStreamReader::new(input, policy).expect("policy is valid");
    let capture = input.first().is_some_and(|byte| byte & 1 == 1);
    for _ in 0..=policy.max_events {
        match reader.read_event() {
            Ok(XmlStreamEvent::Start(_)) if capture => {
                let capture_limit = max_input_bytes.saturating_add(4_096);
                let _ = reader.capture_current(capture_limit);
            }
            Ok(XmlStreamEvent::Eof) | Err(_) => break,
            Ok(_) => {}
        }
    }
});
