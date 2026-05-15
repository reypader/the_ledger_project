use tokio::sync::mpsc;

use crate::actors::{AcceptResponder, AcceptResult, AcceptedOperation, applier};

#[derive(Debug)]
pub enum Message {
    Ack {
        accepted: AcceptedOperation,
        respond_to: AcceptResponder,
    },
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Message>,
}

impl Handle {
    pub async fn ack(
        &self,
        accepted: AcceptedOperation,
        respond_to: AcceptResponder,
    ) -> Result<(), AcceptResponder> {
        let msg = Message::Ack { accepted, respond_to };
        match self.tx.send(msg).await {
            Ok(()) => Ok(()),
            Err(mpsc::error::SendError(Message::Ack { respond_to, .. })) => Err(respond_to),
        }
    }
}

pub fn spawn(applier: applier::Handle) -> Handle {
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    tokio::spawn(async move {
        println!("writer actor started");
        while let Some(msg) = rx.recv().await {
            match msg {
                Message::Ack { accepted, respond_to } => {
                    let operation_id = accepted.operation_id;
                    let book_ids: Vec<u32> = accepted
                        .ending_balances
                        .iter()
                        .map(|(id, _)| *id)
                        .collect();
                    let _ = respond_to.send(AcceptResult::Accepted(accepted));
                    let _ = applier.apply(operation_id, book_ids).await;
                }
            }
        }
    });
    Handle { tx }
}
