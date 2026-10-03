//! Defaults only: product response headers take precedence; HTTPS termination owns HSTS.
/// Default frame policy.
pub const X_FRAME_OPTIONS_VALUE: &str = "SAMEORIGIN";
/// Default referrer policy.
pub const REFERRER_POLICY_VALUE: &str = "strict-origin-when-cross-origin";
/// Default MIME-sniffing policy.
pub const X_CONTENT_TYPE_OPTIONS_VALUE: &str = "nosniff";
