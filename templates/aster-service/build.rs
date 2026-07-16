//! Build script: inject build metadata and select an embeddable frontend artifact.

use std::env;
use std::fs;
use std::io;
use std::path::Path;

const BUILD_TIME_ENV: &str = "ASTER_BUILD_TIME";
const FRONTEND_DIST_ENV: &str = "ASTER_FRONTEND_DIST_DIR";
const FALLBACK_MARKER_FILE: &str = ".aster-service-frontend-fallback";
const FALLBACK_MARKER_CONTENT: &str = "aster-service-frontend-fallback-v1\n";
const LEGACY_FALLBACK_TEXT: &str = "The service is running. Build";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=frontend-panel/dist");
    println!("cargo:rerun-if-env-changed={BUILD_TIME_ENV}");

    configure_build_time()?;

    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map_err(|error| io::Error::other(format!("missing CARGO_MANIFEST_DIR: {error}")))?;
    let dist_path = Path::new(&manifest_dir).join("frontend-panel/dist");
    let out_dir = env::var("OUT_DIR")
        .map_err(|error| io::Error::other(format!("missing OUT_DIR: {error}")))?;
    let fallback_dist_path = Path::new(&out_dir).join("frontend-dist-fallback");
    let profile = env::var("PROFILE")
        .map_err(|error| io::Error::other(format!("missing PROFILE: {error}")))?;

    let selected_dist_path = match frontend_dist_state(&dist_path)? {
        FrontendDistState::Real => dist_path,
        FrontendDistState::Missing if fallback_allowed(&profile) => {
            eprintln!(
                "Warning: frontend-panel/dist is missing; generating isolated development fallback assets"
            );
            create_fallback_files(&manifest_dir, &fallback_dist_path)?;
            fallback_dist_path
        }
        FrontendDistState::Fallback if fallback_allowed(&profile) => {
            eprintln!(
                "Warning: frontend-panel/dist contains fallback assets; generating a clean isolated fallback"
            );
            create_fallback_files(&manifest_dir, &fallback_dist_path)?;
            fallback_dist_path
        }
        FrontendDistState::Missing | FrontendDistState::Fallback => {
            return Err(io::Error::other(format!(
                "frontend-panel/dist does not contain a production frontend for the {profile} profile; run `cd frontend-panel && bun install --frozen-lockfile && bun run build` before building"
            ))
            .into());
        }
    };

    let selected_dist_path = selected_dist_path.to_str().ok_or_else(|| {
        io::Error::other("selected frontend dist path must contain valid Unicode")
    })?;
    println!("cargo:rustc-env={FRONTEND_DIST_ENV}={selected_dist_path}");

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrontendDistState {
    Missing,
    Fallback,
    Real,
}

fn configure_build_time() -> io::Result<()> {
    let value = match env::var(BUILD_TIME_ENV) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => {
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
        }
        Err(env::VarError::NotUnicode(_)) => {
            return Err(io::Error::other(format!(
                "{BUILD_TIME_ENV} must contain valid Unicode"
            )));
        }
    };

    let value = value.trim();
    if value.is_empty() {
        return Err(io::Error::other(format!(
            "{BUILD_TIME_ENV} must not be empty when set"
        )));
    }
    if value.contains('\r') || value.contains('\n') {
        return Err(io::Error::other(format!(
            "{BUILD_TIME_ENV} must be a single-line value"
        )));
    }
    println!("cargo:rustc-env={BUILD_TIME_ENV}={value}");

    Ok(())
}

fn fallback_allowed(profile: &str) -> bool {
    matches!(profile, "debug" | "test")
}

fn frontend_dist_state(dist_path: &Path) -> io::Result<FrontendDistState> {
    if !dist_path.exists() {
        return Ok(FrontendDistState::Missing);
    }

    if dist_path.join(FALLBACK_MARKER_FILE).exists() {
        return Ok(FrontendDistState::Fallback);
    }

    let index_path = dist_path.join("index.html");
    if !index_path.exists() {
        return Ok(FrontendDistState::Missing);
    }

    let index_html = fs::read_to_string(index_path)?;
    if index_html.contains(LEGACY_FALLBACK_TEXT) {
        return Ok(FrontendDistState::Fallback);
    }

    Ok(FrontendDistState::Real)
}

fn create_fallback_files(manifest_dir: &str, dist_path: &Path) -> io::Result<()> {
    if dist_path.exists() {
        fs::remove_dir_all(dist_path)?;
    }
    fs::create_dir_all(dist_path.join("assets"))?;
    fs::write(
        dist_path.join(FALLBACK_MARKER_FILE),
        FALLBACK_MARKER_CONTENT,
    )?;

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
        with the product frontend. Use <code>bun install --frozen-lockfile</code> before building.
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
