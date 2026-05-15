pub mod acceptor;
pub mod applier;
pub mod book_manager;
pub mod provisioner;
pub mod writer;

use std::sync::Arc;

use tokio::sync::oneshot;

use crate::proto::{AccountType, BalanceType};

#[derive(Debug, Clone, Copy)]
pub struct BookState {
    pub id: u32,
    pub accounting_type: AccountType,
    pub balance_type: BalanceType,
    pub allow_overdraft: bool,
    pub projected_balance: i128,
    pub latest_segment: u32,
    pub latest_offset: u32,
}

#[derive(Debug)]
pub struct PreprocessedBook {
    pub book_state: Arc<BookState>,
    pub incoming_total: i64,
    pub ops: Vec<PreprocessedEntry>,
}

#[derive(Debug, Clone, Copy)]
pub struct PreprocessedEntry {
    pub signed_amount: i64,
    pub ledger_code: [u8; 8],
}

#[derive(Debug)]
pub struct AcceptedOperation {
    pub operation_id: u64,
    pub timestamp_ns: u128,
    pub ending_balances: Vec<(u32, i64)>,
}

#[derive(Debug)]
pub enum AcceptResult {
    Accepted(AcceptedOperation),
    Rejected { book_id: u32 },
    Failure(String),
}

pub type AcceptResponder = oneshot::Sender<AcceptResult>;

pub struct AppState {
    pub book_manager: book_manager::Handle,
    pub acceptor: acceptor::Handle,
    pub writer: writer::Handle,
    pub applier: applier::Handle,
    pub provisioner: provisioner::Handle,
}
