use std::sync::Arc;

use tonic::{transport::Server, Request, Response, Status};
use tracing::info;

use crate::api::core::DiffService;
use crate::options::{DiffOptions, DisplayOptions};

tonic::include_proto!("difftastic");

#[derive(Clone)]
pub struct GrpcDiffService {
    service: Arc<DiffService>,
}

#[tonic::async_trait]
impl diff_service_server::DiffService for GrpcDiffService {
    async fn diff(
        &self,
        request: Request<DiffRequest>,
    ) -> Result<Response<DiffResponse>, Status> {
        let req = request.into_inner();
        let diff_options = DiffOptions::default();
        let display_options = DisplayOptions::default();

        let language_override = req
            .language_override
            .and_then(|lang| crate::parse::guess_language::language_override_from_name(&lang))
            .and_then(|lo| match lo {
                crate::parse::guess_language::LanguageOverride::Language(lang) => Some(lang),
                crate::parse::guess_language::LanguageOverride::PlainText => None,
            });

        let diff_result = self.service.diff(
            &req.lhs_content,
            &req.rhs_content,
            &req.display_path.unwrap_or_else(|| "unknown".to_string()),
            language_override,
            &[],
            &diff_options,
            &display_options,
        );

        let reply = DiffResponse {
            display_path: diff_result.display_path,
            file_format: format!("{}", diff_result.file_format),
            has_syntactic_changes: diff_result.has_syntactic_changes,
            has_byte_changes: diff_result.has_byte_changes.is_some(),
            lhs_byte_len: diff_result.has_byte_changes.map(|(lhs, _)| lhs as u64),
            rhs_byte_len: diff_result.has_byte_changes.map(|(_, rhs)| rhs as u64),
            hunks: vec![],
        };

        Ok(Response::new(reply))
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let reply = HealthResponse {
            status: "OK".to_string(),
            version: 1,
        };
        Ok(Response::new(reply))
    }

    async fn list_languages(
        &self,
        _request: Request<ListLanguagesRequest>,
    ) -> Result<Response<ListLanguagesResponse>, Status> {
        use strum::IntoEnumIterator;
        let languages: Vec<String> = crate::parse::guess_language::Language::iter()
            .map(|lang| crate::parse::guess_language::language_name(lang).to_string())
            .collect();

        let reply = ListLanguagesResponse { languages };
        Ok(Response::new(reply))
    }
}

pub fn run_grpc() {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
    rt.block_on(async {
        let service = Arc::new(DiffService::new());
        start(service).await;
    });
}

pub async fn start(service: Arc<DiffService>) {
    let grpc_service = GrpcDiffService { service };

    let addr = "[::]:50051".parse().expect("Failed to parse address");
    info!("gRPC server listening on [::]:50051");

    Server::builder()
        .add_service(diff_service_server::DiffServiceServer::new(grpc_service))
        .serve(addr)
        .await
        .expect("gRPC server error");
}
