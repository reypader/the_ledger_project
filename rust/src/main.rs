mod actors;
mod error;
mod proto;
mod service;

use tonic::transport::Server;

use crate::actors::{AppState, acceptor, applier, book_manager, provisioner, writer};
use crate::proto::ledger_service_server::LedgerServiceServer;
use crate::service::LedgerApi;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provisioner = provisioner::spawn();
    let applier = applier::spawn();
    let writer = writer::spawn(applier.clone());
    let acceptor = acceptor::spawn(writer.clone());
    let book_manager = book_manager::spawn();

    let state = AppState {
        book_manager,
        acceptor,
        writer,
        applier,
        provisioner,
    };

    let addr = "127.0.0.1:50051".parse()?;
    println!("ledger-api listening on {addr}");

    Server::builder()
        .add_service(LedgerServiceServer::new(LedgerApi::new(state)))
        .serve(addr)
        .await?;

    Ok(())
}
