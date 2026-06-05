mod containerd_runtime;

use billow_api::api::echo_service_server::{EchoService, EchoServiceServer};
use billow_api::api::task_service_server::{TaskService, TaskServiceServer};
use billow_api::api::{EchoRequest, EchoResponse, RunRequest, RunResponse};
use containerd_runtime::ContainerdRuntime;
use std::sync::Arc;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

#[derive(Clone, Default)]
struct Agent {
    runtime: Arc<ContainerdRuntime>,
}

#[tonic::async_trait]
impl EchoService for Agent {
    async fn echo(&self, request: Request<EchoRequest>) -> Result<Response<EchoResponse>, Status> {
        Ok(Response::new(EchoResponse {
            message: request.into_inner().message,
        }))
    }
}

#[tonic::async_trait]
impl TaskService for Agent {
    async fn run(&self, request: Request<RunRequest>) -> Result<Response<RunResponse>, Status> {
        let image = request.into_inner().image;
        if image.trim().is_empty() {
            return Err(Status::invalid_argument("image reference cannot be empty"));
        }

        self.runtime
            .run(&image)
            .await
            .map(Response::new)
            .map_err(|error| Status::internal(error.to_string()))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50052".parse()?;
    let agent = Agent::default();

    Server::builder()
        .add_service(EchoServiceServer::new(agent.clone()))
        .add_service(TaskServiceServer::new(agent))
        .serve(addr)
        .await?;

    Ok(())
}
