#![no_main]

use aster_forge_xml::{
    BorrowedDocument, XmlSafetyPolicy, XmlStreamWriter, XmlWriteOptions, validate_xml_input,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    exercise_writer(input);
});

fn exercise_writer(input: &[u8]) {
    let options = XmlWriteOptions::new()
        .max_output_bytes(1024 * 1024)
        .max_depth(64)
        .max_attributes_per_element(64);
    let mut writer = XmlStreamWriter::with_options(Vec::new(), options).expect("valid options");
    writer.start("root").expect("static root is valid");

    for (index, chunk) in input.chunks(32).take(256).enumerate() {
        let value = String::from_utf8_lossy(chunk);
        match index % 5 {
            0 => {
                let _ = writer.text(&value);
            }
            1 => {
                let _ = writer.comment(&value);
            }
            2 => {
                let _ = writer.cdata(&value);
            }
            3 => {
                let _ = writer.processing_instruction(&value, Some("value"));
            }
            _ => {
                let _ = writer.empty_element(&value, [("value", value.as_ref())]);
            }
        }
    }

    writer
        .end_element()
        .expect("failed writes do not corrupt root state");
    let output = writer.finish().expect("complete root");
    let policy = XmlSafetyPolicy {
        max_input_bytes: output.len().max(1),
        max_text_bytes: output.len().max(1),
        ..XmlSafetyPolicy::untrusted()
    };
    validate_xml_input(&output, policy).expect("successful writer output validates");
    BorrowedDocument::parse_with_options(
        output.as_slice(),
        &aster_forge_xml::ParseOptions::new().safety_policy(policy),
    )
    .expect("successful writer output reparses");
}
