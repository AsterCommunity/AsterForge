//! Binary entrypoint.
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::unreachable,
        clippy::expect_used,
        clippy::panic,
        clippy::unimplemented,
        clippy::todo
    )
)]

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "jemalloc", not(target_env = "msvc"), target_os = "linux"))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static malloc_conf: Option<&'static std::ffi::c_char> = Some(unsafe {
    union Conf {
        bytes: &'static u8,
        ptr: &'static std::ffi::c_char,
    }

    Conf {
        bytes: &b"narenas:1,dirty_decay_ms:1000,muzzy_decay_ms:1000,background_thread:true\0"[0],
    }
    .ptr
});

#[cfg(all(
    feature = "jemalloc",
    not(target_env = "msvc"),
    any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    )
))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static malloc_conf: Option<&'static std::ffi::c_char> = Some(unsafe {
    union Conf {
        bytes: &'static u8,
        ptr: &'static std::ffi::c_char,
    }

    Conf {
        bytes: &b"narenas:1,dirty_decay_ms:1000,muzzy_decay_ms:1000\0"[0],
    }
    .ptr
});

#[cfg(all(debug_assertions, not(feature = "jemalloc")))]
#[global_allocator]
static GLOBAL: aster_forge_alloc::TrackingAlloc = aster_forge_alloc::TrackingAlloc;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    {{crate_name}}::runtime::entrypoint::run().await
}
