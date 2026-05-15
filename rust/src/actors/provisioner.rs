use tokio::sync::mpsc;

use crate::error::ServiceError;

#[derive(Debug)]
pub enum Message {
    PreProvisionOperationSegment { segment_number: u32 },
    PreProvisionBookSegment { book_id: u32, segment_number: u32 },
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Message>,
}

impl Handle {
    pub async fn pre_provision_operation(&self, segment_number: u32) -> Result<(), ServiceError> {
        self.tx
            .send(Message::PreProvisionOperationSegment { segment_number })
            .await
            .map_err(|_| ServiceError::Internal("provisioner unavailable".into()))
    }

    pub async fn pre_provision_book(
        &self,
        book_id: u32,
        segment_number: u32,
    ) -> Result<(), ServiceError> {
        self.tx
            .send(Message::PreProvisionBookSegment { book_id, segment_number })
            .await
            .map_err(|_| ServiceError::Internal("provisioner unavailable".into()))
    }
}

pub fn spawn() -> Handle {
    let (tx, mut rx) = mpsc::channel::<Message>(256);
    tokio::spawn(async move {
        println!("provisioner actor started");
        while rx.recv().await.is_some() {
            // stub drops all provisioning requests
        }
    });
    Handle { tx }
}
