//! CPU benchmarks for the XML implementations relevant to Aster products.

mod support;

use std::hint::black_box;
use std::time::Duration;

use aster_forge_utils::numbers::usize_to_u64;
use aster_forge_xml::{BorrowedDocument, XmlSafetyPolicy, validate_xml_input};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use support::{
    fixtures, validate_forge_stream, walk_forge_stream, walk_quick_xml_events,
    walk_quick_xml_ns_buffered, write_forge_multistatus, write_quick_xml_multistatus,
    write_xmltree_multistatus,
};

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("xml/parse");
    group
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20);
    for (name, input) in fixtures()
        .into_iter()
        .filter(|(name, _)| *name != "multistatus_10000")
    {
        group.throughput(Throughput::Bytes(
            usize_to_u64(input.len(), "benchmark input length")
                .expect("benchmark input length should fit in u64"),
        ));
        group.bench_with_input(BenchmarkId::new("forge_arena", name), &input, |b, input| {
            b.iter(|| {
                BorrowedDocument::parse(black_box(input.as_slice())).expect("fixture should parse")
            });
        });
        group.bench_with_input(
            BenchmarkId::new("roxmltree_borrowed", name),
            &input,
            |b, input| {
                let input = std::str::from_utf8(input).expect("fixture should be UTF-8");
                b.iter(|| {
                    roxmltree::Document::parse(black_box(input)).expect("fixture should parse")
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("xmltree_owned", name),
            &input,
            |b, input| {
                b.iter(|| {
                    xmltree::Element::parse(black_box(input.as_slice()))
                        .expect("fixture should parse")
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("validate_plus_xmltree", name),
            &input,
            |b, input| {
                b.iter(|| {
                    validate_xml_input(black_box(input), XmlSafetyPolicy::untrusted())
                        .expect("fixture should validate");
                    xmltree::Element::parse(input.as_slice()).expect("fixture should parse")
                });
            },
        );
        group.bench_with_input(
            BenchmarkId::new("forge_stream_reader", name),
            &input,
            |b, input| b.iter(|| walk_forge_stream(black_box(input))),
        );
        group.bench_with_input(
            BenchmarkId::new("forge_stream_validation", name),
            &input,
            |b, input| b.iter(|| validate_forge_stream(black_box(input))),
        );
        group.bench_with_input(
            BenchmarkId::new("quick_xml_ns_buffered_decoded", name),
            &input,
            |b, input| b.iter(|| walk_quick_xml_ns_buffered(black_box(input))),
        );
        group.bench_with_input(
            BenchmarkId::new("quick_xml_borrowed_events", name),
            &input,
            |b, input| b.iter(|| walk_quick_xml_events(black_box(input))),
        );
    }
    group.finish();
}

fn bench_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("xml/write");
    group
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20);
    let mut xmltree_options = xmltree::EmitterConfig::new();
    xmltree_options.perform_indent = false;
    xmltree_options.write_document_declaration = false;
    xmltree_options.pad_self_closing = false;
    xmltree_options.autopad_comments = false;

    for (name, input) in fixtures()
        .into_iter()
        .filter(|(name, _)| *name != "multistatus_10000")
    {
        let arena = BorrowedDocument::parse(input.as_slice()).expect("fixture should parse");
        let xmltree = xmltree::Element::parse(input.as_slice()).expect("fixture should parse");
        group.throughput(Throughput::Bytes(
            usize_to_u64(input.len(), "benchmark input length")
                .expect("benchmark input length should fit in u64"),
        ));
        group.bench_function(BenchmarkId::new("arena_original", name), |b| {
            b.iter(|| {
                let mut output = Vec::with_capacity(input.len());
                arena
                    .write_original(&mut output)
                    .expect("fixture should write");
                black_box(output)
            });
        });
        group.bench_function(BenchmarkId::new("xmltree_compact", name), |b| {
            b.iter(|| {
                let mut output = Vec::with_capacity(input.len());
                xmltree
                    .write_with_config(&mut output, xmltree_options.clone())
                    .expect("fixture should write");
                black_box(output)
            });
        });
    }
    group.finish();

    let responses = 1_000usize;
    let expected_bytes = support::multistatus(responses).len();
    let forge_output = write_forge_multistatus(Vec::new(), responses);
    let quick_output = write_quick_xml_multistatus(Vec::new(), responses);
    let xmltree_output = write_xmltree_multistatus(responses);
    BorrowedDocument::parse(forge_output.as_slice()).expect("Forge output should parse");
    BorrowedDocument::parse(quick_output.as_slice()).expect("quick-xml output should parse");
    BorrowedDocument::parse(xmltree_output.as_slice()).expect("xmltree output should parse");

    let mut group = c.benchmark_group("xml/generate_multistatus");
    group
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2))
        .sample_size(20)
        .throughput(Throughput::Bytes(
            usize_to_u64(expected_bytes, "benchmark output length")
                .expect("benchmark output length should fit in u64"),
        ));
    group.bench_function("forge_stream_writer/1000", |b| {
        b.iter(|| black_box(write_forge_multistatus(Vec::new(), responses)));
    });
    group.bench_function("quick_xml_writer/1000", |b| {
        b.iter(|| black_box(write_quick_xml_multistatus(Vec::new(), responses)));
    });
    group.bench_function("xmltree_build_and_write/1000", |b| {
        b.iter(|| black_box(write_xmltree_multistatus(responses)));
    });
    group.finish();
}

criterion_group!(benches, bench_parse, bench_write);
criterion_main!(benches);
