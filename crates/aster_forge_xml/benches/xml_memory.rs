//! Allocation and peak-RSS probe for XML parsing and compact writing.

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use aster_forge_utils::numbers::i64_to_u64;
use aster_forge_xml::{BorrowedDocument, ValidatedXml, XmlSafetyPolicy, validate_xml_input};
use support::{
    fixtures, validate_forge_stream, walk_forge_stream, walk_quick_xml_events,
    walk_quick_xml_ns_buffered, write_forge_multistatus, write_quick_xml_multistatus,
    write_xmltree_multistatus,
};

struct CountingAllocator;

static ENABLED: AtomicBool = AtomicBool::new(false);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwards the exact layout to the system allocator.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwards the exact layout to the system allocator.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        record_deallocation(layout.size());
        // SAFETY: the pointer and layout came from this allocator.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the pointer and old layout came from this allocator.
        let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
        if !new_pointer.is_null() && ENABLED.load(Ordering::Relaxed) {
            record_deallocation(layout.size());
            record_allocation(new_size);
        }
        new_pointer
    }
}

fn record_allocation(size: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    TOTAL.fetch_add(size, Ordering::Relaxed);
    let live = LIVE.fetch_add(size, Ordering::Relaxed).saturating_add(size);
    PEAK.fetch_max(live, Ordering::Relaxed);
}

fn record_deallocation(size: usize) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let _ = LIVE.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size))
    });
}

#[derive(Clone, Copy)]
struct AllocationSnapshot {
    allocations: usize,
    total_bytes: usize,
    peak_live_bytes: usize,
    retained_bytes: usize,
}

fn begin_measurement() {
    LIVE.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    TOTAL.store(0, Ordering::Relaxed);
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ENABLED.store(true, Ordering::SeqCst);
}

fn finish_measurement() -> AllocationSnapshot {
    ENABLED.store(false, Ordering::SeqCst);
    AllocationSnapshot {
        allocations: ALLOCATIONS.load(Ordering::Relaxed),
        total_bytes: TOTAL.load(Ordering::Relaxed),
        peak_live_bytes: PEAK.load(Ordering::Relaxed),
        retained_bytes: LIVE.load(Ordering::Relaxed),
    }
}

fn run_child(mode: &str, fixture_name: &str) {
    let input = fixtures()
        .into_iter()
        .find_map(|(name, input)| (name == fixture_name).then_some(input))
        .unwrap_or_else(|| panic!("unknown fixture `{fixture_name}`"));
    begin_measurement();
    match mode {
        "forge_arena_borrowed" => {
            let document = BorrowedDocument::parse(input.as_slice()).expect("fixture should parse");
            black_box(&document);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "forge_validated_owned" => {
            let document = ValidatedXml::new(input.clone()).expect("fixture should parse");
            black_box(&document);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "xmltree_owned" => {
            let document = xmltree::Element::parse(input.as_slice()).expect("fixture should parse");
            black_box(&document);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "roxmltree_borrowed" => {
            let input = std::str::from_utf8(&input).expect("fixture should be UTF-8");
            let document = roxmltree::Document::parse(input).expect("fixture should parse");
            black_box(&document);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "validate_plus_xmltree" => {
            validate_xml_input(&input, XmlSafetyPolicy::untrusted())
                .expect("fixture should validate");
            let document = xmltree::Element::parse(input.as_slice()).expect("fixture should parse");
            black_box(&document);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "quick_xml_borrowed_events" => {
            black_box(walk_quick_xml_events(&input));
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "quick_xml_ns_buffered_decoded" => {
            black_box(walk_quick_xml_ns_buffered(&input));
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "forge_stream_reader" => {
            black_box(walk_forge_stream(&input));
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "forge_stream_validation" => {
            black_box(validate_forge_stream(&input));
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "forge_arena_original" => {
            ENABLED.store(false, Ordering::SeqCst);
            let document = BorrowedDocument::parse(input.as_slice()).expect("fixture should parse");
            begin_measurement();
            let mut output = Vec::with_capacity(input.len());
            document
                .write_original(&mut output)
                .expect("fixture should write");
            black_box(&output);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "xmltree_write" => {
            ENABLED.store(false, Ordering::SeqCst);
            let document = xmltree::Element::parse(input.as_slice()).expect("fixture should parse");
            let mut options = xmltree::EmitterConfig::new();
            options.perform_indent = false;
            options.write_document_declaration = false;
            options.pad_self_closing = false;
            options.autopad_comments = false;
            begin_measurement();
            let mut output = Vec::with_capacity(input.len());
            document
                .write_with_config(&mut output, options)
                .expect("fixture should write");
            black_box(&output);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "forge_stream_writer_vec" => {
            let responses = multistatus_responses(fixture_name);
            let output = write_forge_multistatus(Vec::new(), responses);
            black_box(&output);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "forge_stream_writer_sink" => {
            let responses = multistatus_responses(fixture_name);
            black_box(write_forge_multistatus(std::io::sink(), responses));
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "quick_xml_writer_sink" => {
            let responses = multistatus_responses(fixture_name);
            black_box(write_quick_xml_multistatus(std::io::sink(), responses));
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        "xmltree_build_and_write" => {
            let responses = multistatus_responses(fixture_name);
            let output = write_xmltree_multistatus(responses);
            black_box(&output);
            print_result(mode, fixture_name, input.len(), finish_measurement());
        }
        _ => panic!("unknown mode `{mode}`"),
    }
}

fn multistatus_responses(fixture_name: &str) -> usize {
    match fixture_name {
        "multistatus_1000" => 1_000,
        "multistatus_10000" => 10_000,
        _ => panic!("writer generation requires a multistatus fixture"),
    }
}

fn print_result(
    mode: &str,
    fixture_name: &str,
    input_bytes: usize,
    allocation: AllocationSnapshot,
) {
    println!(
        "{mode}\t{fixture_name}\t{input_bytes}\t{}\t{}\t{}\t{}\t{}",
        allocation.allocations,
        allocation.total_bytes,
        allocation.peak_live_bytes,
        allocation.retained_bytes,
        peak_rss_bytes(),
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage initializes the supplied rusage structure on success.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result != 0 {
        return 0;
    }
    // SAFETY: getrusage returned success, so the structure is initialized.
    let usage = unsafe { usage.assume_init() };
    #[cfg(target_os = "macos")]
    {
        i64_to_u64(usage.ru_maxrss, "peak RSS").unwrap_or(0)
    }
    #[cfg(target_os = "linux")]
    {
        i64_to_u64(usage.ru_maxrss, "peak RSS")
            .unwrap_or(0)
            .saturating_mul(1024)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn peak_rss_bytes() -> u64 {
    0
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.iter().any(|argument| argument == "--test") {
        return;
    }
    if let [_, child, mode, fixture_name] = arguments.as_slice()
        && child == "--child"
    {
        run_child(mode, fixture_name);
        return;
    }

    println!(
        "mode\tfixture\tinput_bytes\tallocations\ttotal_allocated\tpeak_live_heap\tretained_heap\tpeak_rss"
    );
    let executable = std::env::current_exe().expect("current benchmark executable");
    for fixture_name in ["propfind", "wopi", "multistatus_1000", "multistatus_10000"] {
        for mode in [
            "forge_arena_borrowed",
            "forge_validated_owned",
            "roxmltree_borrowed",
            "xmltree_owned",
            "validate_plus_xmltree",
            "forge_stream_reader",
            "forge_stream_validation",
            "quick_xml_ns_buffered_decoded",
            "quick_xml_borrowed_events",
            "forge_arena_original",
            "xmltree_write",
        ] {
            let output = Command::new(&executable)
                .args(["--child", mode, fixture_name])
                .output()
                .expect("memory benchmark child should start");
            assert!(output.status.success(), "memory benchmark child failed");
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        if fixture_name.starts_with("multistatus_") {
            for mode in [
                "forge_stream_writer_vec",
                "forge_stream_writer_sink",
                "quick_xml_writer_sink",
                "xmltree_build_and_write",
            ] {
                let output = Command::new(&executable)
                    .args(["--child", mode, fixture_name])
                    .output()
                    .expect("memory benchmark child should start");
                assert!(output.status.success(), "memory benchmark child failed");
                print!("{}", String::from_utf8_lossy(&output.stdout));
            }
        }
    }
}
