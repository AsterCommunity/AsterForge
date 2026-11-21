//! Runs the memory cloud drive example fixture as a product-worker integration test on Linux.

#![cfg(target_os = "linux")]
#![expect(
    dead_code,
    reason = "the integration test includes the complete executable fixture"
)]

// Keep the synthetic product worker tests next to the example implementation without making the
// example's product-owned state types part of the public Linux crate.
#[path = "../examples/memory_cloud_drive.rs"]
mod memory_cloud_drive;
