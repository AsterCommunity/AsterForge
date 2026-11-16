//! Repository release automation; product runtime crates do not depend on this tool.
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented
)]

pub mod archive;
pub mod checks;
pub mod cli;
pub mod graph;
pub mod lockfile;
pub mod manifest;
pub mod model;
pub mod output;
pub mod prepare;
pub mod publish;
pub mod registry;
