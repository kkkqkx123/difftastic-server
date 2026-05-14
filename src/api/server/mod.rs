#[cfg(feature = "http")]
mod http_server;

#[cfg(feature = "grpc")]
mod grpc_server;

#[cfg(feature = "http")]
pub use http_server::run_http;

#[cfg(feature = "grpc")]
pub use grpc_server::run_grpc;

#[cfg(feature = "grpc")]
pub fn run() {
    use std::sync::Arc;

    use crate::api::core::DiffService;

    let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    rt.block_on(async {
        let service = Arc::new(DiffService::new());
        let http_service = service.clone();

        let http_task = tokio::spawn(async move {
            http_server::start(http_service).await;
        });

        let grpc_task = tokio::spawn(async move {
            grpc_server::start(service).await;
        });

        let _ = tokio::join!(http_task, grpc_task);
    });
}

#[cfg(all(feature = "http", not(feature = "grpc")))]
pub fn run() {
    run_http();
}
