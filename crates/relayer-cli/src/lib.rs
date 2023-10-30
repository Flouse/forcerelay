//! Forcerelay: IBC Relayer CLI built in Rust
//!
//! The Forcerelay binary is a wrapper over the [ibc-relayer] library. This binary builds on
//! the [Abscissa] framework.
//!
//! For a comprehensive guide to using Forcerelay, the authoritative resource is
//! at [hermes.informal.systems].
//!
//! [ibc-relayer]: https://docs.rs/ibc-relayer
//! [Abscissa]: https://github.com/iqlusioninc/abscissa
//! [hermes.informal.systems]: https://hermes.informal.systems

// Tip: Deny warnings with `RUSTFLAGS="-D warnings"` environment variable in CI

#![forbid(unsafe_code)]
#![deny(
    rust_2018_idioms,
    trivial_casts,
    unused_lifetimes,
    unused_qualifications
)]
#![allow(deprecated)]
#![allow(clippy::redundant_closure_call)]

extern crate alloc;

pub mod application;
pub mod chain_registry;
pub mod cli_utils;
pub mod commands;
pub mod components;
pub mod conclude;
pub mod config;
pub mod entry;
pub mod error;
pub mod prelude;

/// The path to the default configuration file, relative to the home directory.
pub const DEFAULT_CONFIG_PATH: &str = ".forcerelay/config.toml";
