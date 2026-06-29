//! Build script: keep an embeddable frontend artifact available for Rust builds.

use std::env;
use std::fs;
use std::io;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=frontend-panel/dist");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map_err(|error| io::Error::other(format!("missing CARGO_MANIFEST_DIR: {error}")))?;
    let dist_path = Path::new(&manifest_dir).join("frontend-panel/dist");

    if !dist_path.exists() {
        eprintln!("Warning: frontend-panel/dist directory not found.");
        eprintln!("Please build the frontend first:");
        eprintln!("  cd frontend-panel && bun install && bun run build");

        create_fallback_files(&manifest_dir, &dist_path)?;
    }

    Ok(())
}

fn create_fallback_files(manifest_dir: &str, dist_path: &Path) -> io::Result<()> {
    fs::create_dir_all(dist_path.join("assets"))?;

    let fallback_html = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <link rel="icon" type="image/svg+xml" href="%ASTER_SERVICE_FAVICON_URL%" />
    <link rel="apple-touch-icon" href="%ASTER_SERVICE_FAVICON_URL%" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <meta name="description" content="%ASTER_SERVICE_DESCRIPTION%" />
    <meta http-equiv="Content-Security-Policy" content="%ASTER_SERVICE_CSP%" />
    <meta name="aster-service-version" content="%ASTER_SERVICE_VERSION%" />
    <title>%ASTER_SERVICE_TITLE%</title>
    <style>
      :root {
        color-scheme: light dark;
        font-family:
          Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI",
          sans-serif;
        background: #f6f7f9;
        color: #172026;
      }

      body {
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
      }

      main {
        width: min(640px, calc(100vw - 48px));
        border: 1px solid #d8dee6;
        border-radius: 8px;
        background: #ffffff;
        padding: 32px;
        box-shadow: 0 18px 48px rgb(31 41 55 / 10%);
      }

      h1 {
        margin: 0 0 12px;
        font-size: 28px;
        line-height: 1.15;
      }

      p {
        margin: 0;
        color: #52606d;
        line-height: 1.65;
      }

      code {
        border-radius: 5px;
        background: #edf2f7;
        padding: 2px 5px;
      }

      @media (prefers-color-scheme: dark) {
        :root {
          background: #101418;
          color: #f4f7fb;
        }

        main {
          border-color: #2f3a45;
          background: #171d23;
          box-shadow: 0 18px 48px rgb(0 0 0 / 30%);
        }

        p {
          color: #aeb8c4;
        }

        code {
          background: #26313b;
        }
      }
    </style>
  </head>
  <body>
    <main>
      <h1>%ASTER_SERVICE_TITLE%</h1>
      <p>
        The service is running. Build <code>frontend-panel</code> to replace this fallback page
        with the product frontend.
      </p>
    </main>
  </body>
</html>
"#;

    fs::write(dist_path.join("index.html"), fallback_html)?;

    let favicon_path = Path::new(manifest_dir).join("frontend-panel/public/favicon.svg");
    let favicon = fs::read(&favicon_path).unwrap_or_else(|_| fallback_favicon().to_vec());
    fs::write(dist_path.join("favicon.svg"), favicon)?;
    Ok(())
}

fn fallback_favicon() -> &'static [u8] {
    br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><rect width="64" height="64" rx="14" fill="#0f766e"/><path d="M18 42 31 14h4l11 28h-7l-2-6H26l-2 6h-6Zm11-12h6l-3-8-3 8Z" fill="#fff"/></svg>"##
}
