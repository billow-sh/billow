use billow_api::api::echo_service_server::{EchoService, EchoServiceServer};
use billow_api::api::task_service_server::{TaskService, TaskServiceServer};
use billow_api::api::{EchoRequest, EchoResponse, RunRequest};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

#[derive(Default)]
struct Agent;

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
    async fn run(&self, request: Request<RunRequest>) -> Result<Response<()>, Status> {
        println!("Running {}...", request.into_inner().image);

        Ok(Response::new(()))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50052".parse()?;

    Server::builder()
        .add_service(EchoServiceServer::new(Agent))
        .add_service(TaskServiceServer::new(Agent))
        .serve(addr)
        .await?;

    Ok(())
}
