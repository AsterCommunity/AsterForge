#[cfg(target_os = "linux")]
mod linux_example {
    include!("memory_cloud_drive/common.rs");
    include!("memory_cloud_drive/persistence.rs");
    include!("memory_cloud_drive/writeback.rs");
    include!("memory_cloud_drive/namespace.rs");
    include!("memory_cloud_drive/mutation_helpers.rs");
    include!("memory_cloud_drive/mutation.rs");
    include!("memory_cloud_drive/upload.rs");
    include!("memory_cloud_drive/remote.rs");
    include!("memory_cloud_drive/fixture.rs");
    include!("memory_cloud_drive/fixture_helpers.rs");
    include!("memory_cloud_drive/runtime.rs");
    include!("memory_cloud_drive/tests.rs");
    include!("memory_cloud_drive/namespace_tests.rs");
}

#[cfg(target_os = "linux")]
fn main() -> linux_example::ExampleResult<()> {
    linux_example::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("memory_cloud_drive requires Linux and a usable /dev/fuse device");
}
