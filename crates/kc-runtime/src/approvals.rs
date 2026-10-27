//! Human-in-the-loop approval plumbing.

use kc_core::approval::{ApprovalDecision, ApprovalKind, ApprovalRequest};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub struct ApprovalRegistry {
    pending: Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>,
}

impl ApprovalRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn insert(&self, id: &str) -> oneshot::Receiver<ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("approvals lock").insert(id.to_string(), tx);
        rx
    }

    pub fn resolve(&self, id: &str, decision: ApprovalDecision) -> bool {
        let tx = self.pending.lock().expect("approvals lock").remove(id);
        match tx {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    pub fn cancel_all(&self) {
        let mut guard = self.pending.lock().expect("approvals lock");
        for (_, tx) in guard.drain() {
            let _ = tx.send(ApprovalDecision::Expired);
        }
    }
}

/// Convenience for tools: the request is built by the runtime before calling.
pub type ApprovalRequestFn = Arc<dyn Fn(ApprovalKind, String, String, serde_json::Value) + Send + Sync>;

#[allow(dead_code)]
pub(crate) fn _type_check(_r: ApprovalRequest) {}
