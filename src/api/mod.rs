pub mod core;

#[cfg(feature = "cli")]
mod cli;

#[cfg(any(feature = "http", feature = "grpc"))]
mod server;

#[cfg(all(feature = "cli", not(any(feature = "http", feature = "grpc"))))]
pub(crate) use cli::run;

#[cfg(any(feature = "http", feature = "grpc"))]
pub(crate) use server::run;
