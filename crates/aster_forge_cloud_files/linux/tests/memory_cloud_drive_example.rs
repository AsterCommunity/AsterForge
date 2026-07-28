#![cfg(target_os = "linux")]
#![allow(dead_code)]

// Keep the synthetic product worker tests next to the example implementation without making the
// example's product-owned state types part of the public Linux crate.
#[path = "../examples/memory_cloud_drive.rs"]
mod memory_cloud_drive;
