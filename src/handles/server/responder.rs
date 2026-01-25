//! Responder and `ServiceEvent` types for server-side handling

use tokio::sync::oneshot;

use crate::error::{Error, Result};
use crate::{ClientInfo, EventgroupId, MethodId, ReturnCode};

/// Events received by an offered service
#[derive(Debug)]
pub enum ServiceEvent {
    /// A method was called
    Call {
        method: MethodId,
        payload: bytes::Bytes,
        client: ClientInfo,
        responder: Responder,
    },
    /// A fire-and-forget method was called (no response expected)
    FireForget {
        method: MethodId,
        payload: bytes::Bytes,
        client: ClientInfo,
    },
    /// A client wants to subscribe
    Subscribe {
        eventgroup: EventgroupId,
        client: ClientInfo,
    },
    /// A client unsubscribed
    Unsubscribe {
        eventgroup: EventgroupId,
        client: ClientInfo,
    },
}

/// Responder for method calls - must be used to send a response.
#[derive(Debug)]
pub struct Responder {
    pub(crate) response: Option<oneshot::Sender<Result<bytes::Bytes>>>,
}

impl Responder {
    /// Send a successful response.
    pub fn reply(mut self, payload: &[u8]) -> Result<()> {
        if let Some(tx) = self.response.take() {
            let _ = tx.send(Ok(bytes::Bytes::copy_from_slice(payload)));
        }
        Ok(())
    }

    /// Send an error response.
    pub fn reply_error(mut self, code: ReturnCode) -> Result<()> {
        if let Some(tx) = self.response.take() {
            let _ = tx.send(Err(Error::Protocol(crate::error::ProtocolError {
                message: format!("Error: {code:?}"),
            })));
        }
        Ok(())
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        if self.response.is_some() {
            tracing::warn!("Responder dropped without sending response");
            // In debug mode we could panic, but we chose zero-panic
        }
    }
}
