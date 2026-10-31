use aster_forge_xml::XmlStreamWriter;
#[test]
#[ignore = "controlled before/after CPU measurement"]
fn measure_attribute_workload() {
    let names: Vec<_> = (0..512).map(|i| format!("a{i}")).collect();
    let start = std::time::Instant::now();
    for _ in 0..16 {
        let mut writer = XmlStreamWriter::new(Vec::new()).unwrap();
        writer
            .empty_element("r", names.iter().map(|name| (name.as_str(), "value")))
            .unwrap();
        std::hint::black_box(writer.finish().unwrap());
    }
    println!("xml_attributes_us={}", start.elapsed().as_micros());
}
