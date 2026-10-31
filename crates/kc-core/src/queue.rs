//! Messages the user queued while a run was in flight.
//!
//! Two kinds share this shape because they share a queue and a UI:
//!
//! * a **steer** folds into the conversation that is already working ("also
//!   rename the test"), and
//! * a **follow-up** starts a new run once the current one finishes ("then run
//!   the tests").
//!
//! Both are accepted while a run is in flight, both are listed in the composer's
//! pending panel, and both can be **recalled**: taken back out of the queue and
//! put back in the composer, attachments and all. `content` is what the model
//! will see; `draft` and `attachments` are what a recall puts back on screen, so
//! recalling a message with a 40 KB inlined file does not paste that file into
//! the textarea.
//!
//! Nothing here is persisted. A queued message exists only in the runtime's
//! memory until the loop folds it into the conversation, which is what makes a
//! message that never lands simply absent from the transcript instead of a
//! dangling turn.

use serde::{Deserialize, Serialize};

/// How a queued message relates to the run it was accepted during.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueuedKind {
    /// Fold into the run in flight at its next iteration.
    Steer,
    /// Start a new run once the run in flight finishes.
    FollowUp,
}

impl QueuedKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            QueuedKind::Steer => "steer",
            QueuedKind::FollowUp => "follow_up",
        }
    }
}

/// How one attached file will reach the model, mirroring the composer's two
/// pipelines (`web/src/lib/attachments.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentMode {
    /// Small text, pasted into the message body.
    Inline,
    /// Everything else: stored in the workspace and referenced by path.
    Upload,
}

impl AttachmentMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AttachmentMode::Inline => "inline",
            AttachmentMode::Upload => "upload",
        }
    }
}

/// One file the user attached to a queued message.
///
/// The body is **not** duplicated for an upload - it is already in the workspace
/// and referenced by `path`. An *inline* attachment is different: its text was
/// read in the browser from a local file that never reached the server, so
/// `content` is the only copy. Keeping it here is what makes a recall reproduce
/// the same message instead of a chip claiming to carry a file whose body is
/// gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueuedAttachment {
    /// The file's name as the user saw it (`report.pdf`).
    pub name: String,
    /// The path the message references. For an upload this is the server's
    /// answer, which may have been deduplicated.
    pub path: String,
    pub mode: AttachmentMode,
    /// Size in bytes, for the chip's subtitle. `0` means "not recorded".
    #[serde(default)]
    pub size: u64,
    /// The inlined text, for `mode == Inline`; absent for an upload. It is also
    /// already inside [`QueuedMessage::content`], which is what the model sees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// One message waiting for a run to pick it up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// Stable id: `steer_…` or `followup_…`. What a recall names.
    pub id: String,
    pub kind: QueuedKind,
    /// The exact text the model will see once this lands: the draft with one
    /// rendered block per attachment, in the same format a plain send uses.
    pub content: String,
    /// What the user typed. Absent when it is the same as `content`, which is
    /// the normal case for a message with no attachments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<QueuedAttachment>,
    /// Unix ms, for the panel's "queued 12s ago".
    pub accepted_at: i64,
}

impl QueuedMessage {
    /// The draft to put back in the composer when this is recalled.
    pub fn recall_text(&self) -> &str {
        self.draft.as_deref().unwrap_or(&self.content)
    }

    /// Browser view. The one shape the HTTP API and the event stream use.
    pub fn public_view(&self) -> serde_json::Value {
        let mut view = serde_json::json!({
            "id": self.id,
            "kind": self.kind.as_str(),
            "content": self.content,
            "draft": self.recall_text(),
            "accepted_at": self.accepted_at,
        });
        if !self.attachments.is_empty() {
            let attachments: Vec<serde_json::Value> = self
                .attachments
                .iter()
                .map(|attachment| {
                    let mut view = serde_json::json!({
                        "name": attachment.name,
                        "path": attachment.path,
                        "mode": attachment.mode.as_str(),
                        "size": attachment.size,
                    });
                    // Only an inline file needs its body back: an upload is still
                    // on disk at `path`.
                    if let Some(content) = &attachment.content {
                        view["content"] = serde_json::Value::String(content.clone());
                    }
                    view
                })
                .collect();
            view["attachments"] = serde_json::Value::Array(attachments);
        }
        view
    }
}

/// How many files one queued message may carry.
///
/// The composer has no attachment limit of its own; this is the queue's, and it
/// exists so one request cannot park an unbounded chip list in memory for the
/// life of a run. Past it, `POST .../messages?mode=steer` answers the documented
/// `400 bad_request`.
pub const MAX_QUEUED_ATTACHMENTS: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(mode: AttachmentMode) -> QueuedAttachment {
        QueuedAttachment {
            name: "notes.md".into(),
            path: ".karucode/attachments/s_1/notes.md".into(),
            mode,
            size: 812,
            content: match mode {
                AttachmentMode::Inline => Some("# Note\n".into()),
                AttachmentMode::Upload => None,
            },
        }
    }

    #[test]
    fn kinds_and_modes_use_the_wire_spelling() {
        assert_eq!(QueuedKind::Steer.as_str(), "steer");
        assert_eq!(QueuedKind::FollowUp.as_str(), "follow_up");
        assert_eq!(AttachmentMode::Inline.as_str(), "inline");
        assert_eq!(AttachmentMode::Upload.as_str(), "upload");
        assert_eq!(
            serde_json::to_string(&QueuedKind::FollowUp).unwrap(),
            "\"follow_up\""
        );
    }

    #[test]
    fn a_public_view_always_names_the_text_to_recall() {
        let queued = QueuedMessage {
            id: "steer_1".into(),
            kind: QueuedKind::Steer,
            content: "look at this".into(),
            draft: None,
            attachments: Vec::new(),
            accepted_at: 7,
        };
        let view = queued.public_view();
        assert_eq!(view["kind"], "steer");
        // With no attachments the draft and the content are the same text, so
        // the view still answers "what goes back in the composer".
        assert_eq!(view["draft"], "look at this");
        assert_eq!(view["content"], "look at this");
        assert!(view.get("attachments").is_none(), "empty list is omitted");
    }

    #[test]
    fn a_public_view_carries_the_attachment_chips() {
        let queued = QueuedMessage {
            id: "followup_1".into(),
            kind: QueuedKind::FollowUp,
            content: "summarise\n\nAttached 1 file:\n\n### notes.md (inlined, 812 B)".into(),
            draft: Some("summarise".into()),
            attachments: vec![attachment(AttachmentMode::Inline)],
            accepted_at: 9,
        };
        let view = queued.public_view();
        assert_eq!(view["kind"], "follow_up");
        assert_eq!(view["draft"], "summarise");
        assert_eq!(view["attachments"][0]["name"], "notes.md");
        assert_eq!(view["attachments"][0]["mode"], "inline");
        assert_eq!(view["attachments"][0]["size"], 812);
        // An inline file's body comes back, because the browser's copy of the
        // local file is the only one that exists.
        assert_eq!(view["attachments"][0]["content"], "# Note\n");
        // The rendered body is still reported: it is what the model will see.
        assert!(view["content"]
            .as_str()
            .unwrap()
            .contains("### notes.md (inlined, 812 B)"));
    }

    #[test]
    fn an_uploaded_attachment_carries_no_body() {
        let queued = QueuedMessage {
            id: "steer_2".into(),
            kind: QueuedKind::Steer,
            content: "look\n\nAttached 1 file:\n\n### shot.png (uploaded, 812 B)".into(),
            draft: Some("look".into()),
            attachments: vec![attachment(AttachmentMode::Upload)],
            accepted_at: 0,
        };
        let view = queued.public_view();
        assert_eq!(view["attachments"][0]["mode"], "upload");
        // It is still on disk at `path`; duplicating the bytes would be waste.
        assert!(view["attachments"][0].get("content").is_none());
    }

    #[test]
    fn recall_text_prefers_the_draft_and_falls_back_to_the_content() {
        let mut queued = QueuedMessage {
            id: "steer_1".into(),
            kind: QueuedKind::Steer,
            content: "composed".into(),
            draft: Some("typed".into()),
            attachments: Vec::new(),
            accepted_at: 0,
        };
        assert_eq!(queued.recall_text(), "typed");
        queued.draft = None;
        assert_eq!(queued.recall_text(), "composed");
    }
}
