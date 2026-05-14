use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("internal failure: {0}")]
    Internal(String),
}

impl From<ServiceError> for tonic::Status {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::InvalidRequest(msg) => tonic::Status::invalid_argument(msg),
            ServiceError::Internal(msg) => tonic::Status::internal(msg),
        }
    }
}
