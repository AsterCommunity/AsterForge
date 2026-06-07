//! Panic hook setup.

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        tracing::error!(panic = %panic_info, "runtime panic");
        eprintln!("runtime panic: {panic_info}");
    }));
}
