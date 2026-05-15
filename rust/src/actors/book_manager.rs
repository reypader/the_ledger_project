use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::actors::BookState;
use crate::error::ServiceError;
use crate::proto::{AccountType, BalanceType};

#[derive(Debug)]
pub enum Message {
    Create {
        accounting_type: AccountType,
        balance_type: BalanceType,
        allow_overdraft: bool,
        respond_to: oneshot::Sender<u32>,
    },
    LoadBook {
        book_id: u32,
        respond_to: oneshot::Sender<Arc<BookState>>,
    },
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Message>,
}

impl Handle {
    pub async fn create(
        &self,
        accounting_type: AccountType,
        balance_type: BalanceType,
        allow_overdraft: bool,
    ) -> Result<u32, ServiceError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(Message::Create {
                accounting_type,
                balance_type,
                allow_overdraft,
                respond_to,
            })
            .await
            .map_err(|_| ServiceError::Internal("book manager unavailable".into()))?;
        rx.await
            .map_err(|_| ServiceError::Internal("book manager dropped response".into()))
    }

    pub async fn load_book(&self, book_id: u32) -> Result<Arc<BookState>, ServiceError> {
        let (respond_to, rx) = oneshot::channel();
        self.tx
            .send(Message::LoadBook { book_id, respond_to })
            .await
            .map_err(|_| ServiceError::Internal("book manager unavailable".into()))?;
        rx.await
            .map_err(|_| ServiceError::Internal("book manager dropped response".into()))
    }
}

pub fn spawn() -> Handle {
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    tokio::spawn(async move {
        let mut next_id: u32 = 1;
        let mut catalog: HashMap<u32, (AccountType, BalanceType, bool)> = HashMap::new();
        println!("book_manager actor started");
        while let Some(msg) = rx.recv().await {
            match msg {
                Message::Create {
                    accounting_type,
                    balance_type,
                    allow_overdraft,
                    respond_to,
                } => {
                    let id = next_id;
                    next_id += 1;
                    catalog.insert(id, (accounting_type, balance_type, allow_overdraft));
                    let _ = respond_to.send(id);
                }
                Message::LoadBook { book_id, respond_to } => {
                    let (accounting_type, balance_type, allow_overdraft) = catalog
                        .get(&book_id)
                        .copied()
                        .unwrap_or((AccountType::Debit, BalanceType::Available, false));
                    let state = BookState {
                        id: book_id,
                        accounting_type,
                        balance_type,
                        allow_overdraft,
                        projected_balance: 0,
                        latest_segment: 1,
                        latest_offset: 0,
                    };
                    let _ = respond_to.send(Arc::new(state));
                }
            }
        }
    });
    Handle { tx }
}
