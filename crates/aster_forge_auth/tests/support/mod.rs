use std::sync::Once;

pub fn initialize_provider() {
    static STARTUP: Once = Once::new();
    STARTUP.call_once(|| aster_forge_auth::initialize_jwt_crypto_provider().unwrap());
}
