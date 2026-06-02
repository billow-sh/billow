use billow_api::echo::echo_service_server::{EchoService, EchoServiceServer};
use billow_api::echo::{EchoRequest, EchoResponse};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

#[derive(Default)]
struct EchoAgent;

#[tonic::async_trait]
impl EchoService for EchoAgent {
    async fn echo(&self, request: Request<EchoRequest>) -> Result<Response<EchoResponse>, Status> {
        Ok(Response::new(EchoResponse {
            message: request.into_inner().message,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:50051".parse()?;

    Server::builder()
        .add_service(EchoServiceServer::new(EchoAgent))
        .serve(addr)
        .await?;

    Ok(())
}
