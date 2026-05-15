use std::collections::HashMap;
use std::sync::Arc;

use tonic::{Request, Response, Status};

use crate::actors::{
    AcceptResult, AppState, BookState, PreprocessedBook, PreprocessedEntry,
};
use crate::error::ServiceError;
use crate::proto::{
    BookBalance, CreateAccountRequest, CreateAccountResponse, FinancialOperationRequest,
    FinancialOperationResponse, ledger_service_server::LedgerService,
};

pub struct LedgerApi {
    state: AppState,
}

impl LedgerApi {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl LedgerService for LedgerApi {
    async fn create_account(
        &self,
        request: Request<CreateAccountRequest>,
    ) -> Result<Response<CreateAccountResponse>, Status> {
        let command = request.into_inner().command.unwrap_or_default();
        let accounting_type = command.accounting_type();
        let balance_type = command.balance_type();
        let book_id = self
            .state
            .book_manager
            .create(accounting_type, balance_type, command.allow_overdraft)
            .await?;
        Ok(Response::new(CreateAccountResponse { book_id }))
    }

    async fn execute_operation(
        &self,
        request: Request<FinancialOperationRequest>,
    ) -> Result<Response<FinancialOperationResponse>, Status> {
        let entries = request
            .into_inner()
            .command
            .map(|cmd| cmd.entries)
            .unwrap_or_default();

        let mut books: HashMap<u32, Arc<BookState>> = HashMap::new();
        for entry in &entries {
            if !books.contains_key(&entry.book_id) {
                let state = self.state.book_manager.load_book(entry.book_id).await?;
                books.insert(entry.book_id, state);
            }
        }

        let mut grouped: HashMap<u32, (Vec<PreprocessedEntry>, i64)> = HashMap::new();
        for entry in entries {
            let book = books.get(&entry.book_id).ok_or_else(|| {
                ServiceError::Internal(format!("missing book state for {}", entry.book_id))
            })?;
            let entry_account = entry.accounting_type();
            let signed_amount = if entry_account == book.accounting_type {
                entry.amount
            } else {
                -entry.amount
            };
            let ledger_code = encode_ledger_code(&entry.ledger_code);
            let bucket = grouped
                .entry(entry.book_id)
                .or_insert_with(|| (Vec::new(), 0));
            bucket.0.push(PreprocessedEntry { signed_amount, ledger_code });
            bucket.1 = bucket.1.saturating_add(signed_amount);
        }

        let mut preprocessed_books: Vec<PreprocessedBook> = Vec::with_capacity(grouped.len());
        for (book_id, (ops, incoming_total)) in grouped {
            let book_state = books.remove(&book_id).ok_or_else(|| {
                ServiceError::Internal(format!("missing book state for {book_id}"))
            })?;
            preprocessed_books.push(PreprocessedBook { book_state, incoming_total, ops });
        }

        let result = self.state.acceptor.accept(preprocessed_books).await?;

        match result {
            AcceptResult::Accepted(op) => {
                let balances = op
                    .ending_balances
                    .into_iter()
                    .map(|(book_id, ending_balance)| BookBalance { book_id, ending_balance })
                    .collect();
                Ok(Response::new(FinancialOperationResponse {
                    operation_id: op.operation_id,
                    timestamp_ns: op.timestamp_ns as u64,
                    balances,
                }))
            }
            AcceptResult::Rejected { book_id } => {
                Err(ServiceError::InvalidRequest(format!("book {book_id} rejected")).into())
            }
            AcceptResult::Failure(msg) => Err(ServiceError::Internal(msg).into()),
        }
    }
}

fn encode_ledger_code(s: &str) -> [u8; 8] {
    let mut buf = [0u8; 8];
    let bytes = s.as_bytes();
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    buf
}
