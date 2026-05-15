use std::time::SystemTime;

use tokio::sync::{mpsc, oneshot};

use crate::actors::{AcceptResponder, AcceptResult, AcceptedOperation, PreprocessedBook, writer};
use crate::error::ServiceError;

#[derive(Debug)]
pub enum Message {
    Accept {
        books: Vec<PreprocessedBook>,
        respond_to: AcceptResponder,
    },
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Message>,
}

impl Handle {
    pub async fn accept(&self, books: Vec<PreprocessedBook>) -> Result<AcceptResult, ServiceError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(Message::Accept { books, respond_to })
            .await
            .map_err(|_| ServiceError::Internal("acceptor unavailable".into()))?;
        rx.await
            .map_err(|_| ServiceError::Internal("acceptor dropped response".into()))
    }
}

pub fn spawn(writer: writer::Handle) -> Handle {
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    tokio::spawn(async move {
        let mut next_operation_id: u64 = 1;
        println!("acceptor actor started");
        while let Some(msg) = rx.recv().await {
            match msg {
                Message::Accept { books, respond_to } => {
                    let operation_id = next_operation_id;
                    next_operation_id += 1;
                    let timestamp_ns = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    let ending_balances: Vec<(u32, i64)> = books
                        .iter()
                        .map(|book| {
                            let projected = book.book_state.projected_balance;
                            let next = projected.saturating_add(book.incoming_total as i128);
                            let clamped = next.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
                            (book.book_state.id, clamped)
                        })
                        .collect();
                    let accepted = AcceptedOperation {
                        operation_id,
                        timestamp_ns,
                        ending_balances,
                    };
                    if let Err(respond_to) = writer.ack(accepted, respond_to).await {
                        let _ = respond_to.send(AcceptResult::Failure(
                            "writer channel closed".into(),
                        ));
                    }
                }
            }
        }
    });
    Handle { tx }
}
