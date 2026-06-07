//! Build script: inject build time and provide fallback frontend assets.

use std::env;
use std::fs;
use std::io;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=frontend-panel/dist");

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    println!("cargo:rustc-env=ASTER_BUILD_TIME={now}");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map_err(|error| io::Error::other(format!("missing CARGO_MANIFEST_DIR: {error}")))?;
    let dist_path = Path::new(&manifest_dir).join("frontend-panel/dist");

    if !dist_path.exists() {
        create_fallback_files(&dist_path)?;
    }

    Ok(())
}

fn create_fallback_files(dist_path: &Path) -> io::Result<()> {
    fs::create_dir_all(dist_path)?;

    let fallback_html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>AsterForge</title>
    <style>
        :root { color-scheme: dark; }
        body {
            margin: 0;
            min-height: 100vh;
            display: grid;
            place-items: center;
            background: #111827;
            color: #f8fafc;
            font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        }
        main {
            width: min(560px, calc(100vw - 48px));
            border: 1px solid #334155;
            border-radius: 8px;
            padding: 28px;
            background: #0f172a;
        }
        h1 { margin: 0 0 10px; font-size: 28px; letter-spacing: 0; }
        p { color: #cbd5e1; line-height: 1.6; }
        code {
            background: #1e293b;
            border: 1px solid #334155;
            border-radius: 6px;
            padding: 2px 6px;
        }
    </style>
</head>
<body>
    <main>
        <h1>AsterForge</h1>
        <p>The embedded frontend has not been built yet.</p>
        <p>Run <code>cd frontend-panel && bun install && bun run build</code>, then restart the server.</p>
        <p>API health remains available at <code>/health</code> and <code>/health/ready</code>.</p>
    </main>
</body>
</html>"#;

    fs::write(dist_path.join("index.html"), fallback_html)?;
    fs::create_dir_all(dist_path.join("assets"))?;
    Ok(())
}
