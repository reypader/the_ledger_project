mod error;
mod proto;
mod service;

use tonic::transport::Server;

use crate::proto::ledger_service_server::LedgerServiceServer;
use crate::service::LedgerApi;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:50051".parse()?;
    let api = LedgerApi::default();

    println!("ledger-api listening on {addr}");

    Server::builder()
        .add_service(LedgerServiceServer::new(api))
        .serve(addr)
        .await?;

    Ok(())
}
