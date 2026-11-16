//! Minimal terminal error boundary; release logic lives in the library modules.
fn main() {
    if let Err(error) = forge_release::cli::run() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}
