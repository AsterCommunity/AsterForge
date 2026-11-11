//! Run with `--json` or `--silent` to select a public diagnostic policy.
use aster_forge_panic::{DiagnosticSink, PanicHookConfig, PanicOutput, PublicDiagnostic};
use std::io::{self, Write};
use std::sync::Arc;

struct CliJson;
impl DiagnosticSink for CliJson {
    fn emit(&self, diagnostic: &PublicDiagnostic) -> io::Result<()> {
        // The product defines this envelope and deliberately omits raw context.
        writeln!(
            io::stderr(),
            "{}",
            serde_json::json!({
                "code": "internal_error", "timestamp": diagnostic.timestamp,
                "report_written": diagnostic.report_written,
            })
        )
    }
}
fn main() {
    let output = if std::env::args().any(|arg| arg == "--json") {
        PanicOutput::Sink(Arc::new(CliJson))
    } else if std::env::args().any(|arg| arg == "--silent") {
        PanicOutput::Silent
    } else {
        PanicOutput::Human
    };
    aster_forge_panic::install_panic_hook(
        PanicHookConfig::new("example-cli", env!("CARGO_PKG_VERSION"), "").with_output(output),
    );
    // Normal Result handling and process exit policy remain in this executable.
}
