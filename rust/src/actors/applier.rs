use tokio::sync::{mpsc, oneshot};

use crate::error::ServiceError;

#[derive(Debug)]
pub enum Message {
    Apply {
        operation_id: u64,
        book_ids: Vec<u32>,
    },
    Ping {
        respond_to: oneshot::Sender<()>,
    },
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Message>,
}

impl Handle {
    pub async fn apply(
        &self,
        operation_id: u64,
        book_ids: Vec<u32>,
    ) -> Result<(), ServiceError> {
        self.tx
            .send(Message::Apply { operation_id, book_ids })
            .await
            .map_err(|_| ServiceError::Internal("applier unavailable".into()))
    }

    pub async fn ping(&self) -> Result<(), ServiceError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(Message::Ping { respond_to })
            .await
            .map_err(|_| ServiceError::Internal("applier unavailable".into()))?;
        rx.await
            .map_err(|_| ServiceError::Internal("applier dropped response".into()))
    }
}

pub fn spawn() -> Handle {
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    tokio::spawn(async move {
        let mut apply_count: u64 = 0;
        println!("applier actor started");
        while let Some(msg) = rx.recv().await {
            match msg {
                Message::Apply { operation_id, book_ids } => {
                    apply_count += 1;
                    if apply_count == 1 {
                        println!(
                            "applier stub received first Apply (operation_id={operation_id}, books={book_ids:?})"
                        );
                    }
                }
                Message::Ping { respond_to } => {
                    let _ = respond_to.send(());
                }
            }
        }
    });
    Handle { tx }
}
