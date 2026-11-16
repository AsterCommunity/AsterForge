#[expect(
    dead_code,
    reason = "Shared fixtures serve independent integration test crates"
)]
mod support;

use forge_release::{
    archive::verify_batch, checks::refresh_lockfile, manifest::ManifestWorkspace,
    model::CrateVersion,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[test]
fn real_cargo_builds_normalized_batch_archives_against_local_registry_metadata() {
    let directory = support::fixture();
    let root = directory.path();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let server = thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket
                        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                        .unwrap();
                    let mut request = [0; 4096];
                    let size = socket.read(&mut request).unwrap();
                    let request = String::from_utf8_lossy(&request[..size]);
                    let (status, body) = if request.starts_with("GET /config.json ") {
                        ("200 OK", r#"{"dl":"http://127.0.0.1/unused","api":null}"#)
                    } else if request.starts_with("GET /zb/as/zbase ") {
                        (
                            "200 OK",
                            r#"{"name":"zbase","vers":"1.0.1","deps":[],"cksum":"0000000000000000000000000000000000000000000000000000000000000000","features":{"extra":[]},"yanked":false}"#,
                        )
                    } else {
                        ("404 Not Found", "{}")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes());
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(e) => panic!("local registry failed: {e}"),
            }
        }
    });
    std::fs::create_dir(root.join(".cargo")).unwrap();
    std::fs::write(
        root.join(".cargo/config.toml"),
        format!("[source.crates-io]\nregistry = 'sparse+http://{addr}/'\n"),
    )
    .unwrap();
    let mut workspace = ManifestWorkspace::load(root).unwrap();
    workspace.update_pins(&["zbase".into()]).unwrap();
    workspace.write_manifests().unwrap();
    refresh_lockfile(root).unwrap();
    let batch = vec![
        CrateVersion {
            name: "zbase".into(),
            version: "1.0.1".into(),
        },
        CrateVersion {
            name: "aconsumer".into(),
            version: "1.0.0".into(),
        },
    ];
    let result = verify_batch(root, &batch);
    // A file present in the checkout but excluded from the archive must not rescue a build.
    let manifest = root.join("zbase/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace(
        "edition = \"2024\"",
        "edition = \"2024\"\ninclude = [\"Cargo.toml\", \"src/lib.rs\"]",
    );
    std::fs::write(&manifest, text).unwrap();
    std::fs::write(root.join("zbase/hidden.txt"), "checkout-only data").unwrap();
    std::fs::write(
        root.join("zbase/src/lib.rs"),
        "pub const TEXT: &str = include_str!(\"../hidden.txt\");",
    )
    .unwrap();
    let missing_packaged_file = verify_batch(root, &batch);
    stopped.store(true, Ordering::SeqCst);
    server.join().unwrap();
    result.unwrap();
    assert!(
        missing_packaged_file.is_err(),
        "archive verification must not compile workspace-only files"
    );
}
