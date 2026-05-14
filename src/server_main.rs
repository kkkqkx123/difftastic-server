//! Difftastic server - HTTP/gRPC service for structural diff operations.
//!

#![allow(renamed_and_removed_lints)]
#![allow(clippy::type_complexity)]
#![allow(clippy::comparison_to_empty)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::mutable_key_type)]
#![allow(unknown_lints)]
#![allow(clippy::manual_unwrap_or_default)]
#![allow(clippy::implicit_saturating_sub)]
#![allow(clippy::needless_as_bytes)]
#![warn(clippy::str_to_string)]
#![warn(clippy::string_to_string)]
#![warn(clippy::todo)]
#![warn(clippy::dbg_macro)]

mod api;
mod conflicts;
mod constants;
mod diff;
mod display;
mod exit_codes;
mod files;
mod gitattributes;
mod hash;
mod line_parser;
mod lines;
mod options;
mod parse;
mod summary;
mod version;
mod words;

#[macro_use]
extern crate log;

#[cfg(not(any(windows, target_os = "illumos", target_os = "freebsd")))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(any(windows, target_os = "illumos", target_os = "freebsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

extern crate pretty_env_logger;

/// The entrypoint for the server mode.
fn main() {
    pretty_env_logger::try_init_timed_custom_env("DFT_LOG")
        .expect("The logger has not been previously initialized");
    api::run();
}
