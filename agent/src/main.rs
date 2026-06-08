mod containerd_runtime;
mod workload;

use billow_api::api::echo_service_server::{EchoService, EchoServiceServer};
use billow_api::api::workload_service_server::{WorkloadService, WorkloadServiceServer};
use billow_api::api::{
    DeleteWorkloadRequest, EchoRequest, EchoResponse, GetWorkloadLogsRequest,
    GetWorkloadLogsResponse, GetWorkloadRequest, StartWorkloadRequest, StopWorkloadRequest,
    SubmitWorkloadRequest, WorkloadResponse,
};
use containerd_runtime::ContainerdRuntime;
use std::sync::Arc;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use workload::types::WorkloadKind;
use workload::{WorkloadManager, error_status};

#[derive(Clone)]
struct Agent {
    workloads: WorkloadManager,
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
impl WorkloadService for Agent {
    async fn submit(
        &self,
        request: Request<SubmitWorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        let request = request.into_inner();
        let kind = WorkloadKind::from_proto(request.kind).map_err(error_status)?;
        self.workloads
            .submit(kind, request.image)
            .await
            .map(|workload| Response::new(workload.to_proto()))
            .map_err(error_status)
    }

    async fn get(
        &self,
        request: Request<GetWorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        self.workloads
            .get(&request.into_inner().workload_id)
            .map(|workload| Response::new(workload.to_proto()))
            .map_err(error_status)
    }

    async fn start(
        &self,
        request: Request<StartWorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        self.workloads
            .start(&request.into_inner().workload_id)
            .await
            .map(|workload| Response::new(workload.to_proto()))
            .map_err(error_status)
    }

    async fn stop(
        &self,
        request: Request<StopWorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        self.workloads
            .stop(&request.into_inner().workload_id)
            .await
            .map(|workload| Response::new(workload.to_proto()))
            .map_err(error_status)
    }

    async fn delete(
        &self,
        request: Request<DeleteWorkloadRequest>,
    ) -> Result<Response<WorkloadResponse>, Status> {
        self.workloads
            .delete(&request.into_inner().workload_id)
            .await
            .map(|workload| Response::new(workload.to_proto()))
            .map_err(error_status)
    }

    async fn get_logs(
        &self,
        request: Request<GetWorkloadLogsRequest>,
    ) -> Result<Response<GetWorkloadLogsResponse>, Status> {
        let workload_id = request.into_inner().workload_id;
        self.workloads
            .get_logs(&workload_id)
            .await
            .map(|logs| {
                Response::new(GetWorkloadLogsResponse {
                    workload_id,
                    stdout: logs.stdout,
                    stderr: logs.stderr,
                    stdout_truncated: logs.stdout_truncated,
                    stderr_truncated: logs.stderr_truncated,
                })
            })
            .map_err(error_status)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:50052".parse()?;
    let runtime = Arc::new(
        ContainerdRuntime::from_env().map_err(|error| error as Box<dyn std::error::Error>)?,
    );
    let workloads = WorkloadManager::open(runtime)?;
    tokio::spawn(workloads.clone().run_watch_loop());
    tokio::spawn(workloads.clone().run_reconcile_loop());
    let agent = Agent { workloads };

    Server::builder()
        .add_service(EchoServiceServer::new(agent.clone()))
        .add_service(WorkloadServiceServer::new(agent))
        .serve(addr)
        .await?;

    Ok(())
}
