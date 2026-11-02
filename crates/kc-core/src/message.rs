use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    #[default]
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

impl std::str::FromStr for Role {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system" => Ok(Role::System),
            "user" => Ok(Role::User),
            "assistant" => Ok(Role::Assistant),
            "tool" => Ok(Role::Tool),
            _ => Err(()),
        }
    }
}

/// One image carried by a message, ready for a vision-capable endpoint.
///
/// `data` is standard base64 (no `data:` prefix). Only the media types the
/// OpenAI-compatible vision APIs accept are ever produced by the built-in tools
/// (`image/png`, `image/jpeg`, `image/gif`, `image/webp`); a tool that cannot
/// produce a usable image must refuse instead of sending one the endpoint will
/// reject.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImageAttachment {
    pub media_type: String,
    pub data: String,
    /// Decoded size in bytes, kept for accounting and display.
    #[serde(default)]
    pub bytes: u64,
    /// Where it came from (usually the workspace-relative path), for the label
    /// the model reads next to the pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ImageAttachment {
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }

    /// Text shown to the model beside the image.
    pub fn label(&self) -> String {
        let name = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or("image");
        format!("{name} — {}, {} bytes", self.media_type, self.bytes)
    }
}

/// A tool call exactly as the model produced it (id, function name, raw JSON args).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Images this message carries for the model to look at. `content` stays the
    /// text the model reads; the pixels travel beside it because a `tool`
    /// message cannot hold them on the OpenAI-compatible wire format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageAttachment>,
    /// Reasoning content from models that separate thinking from output.
    /// This is preserved across turns for cache efficiency and multi-turn quality,
    /// following the "keep-all" pattern now standard in 2026.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl Message {
    /// Attach images to an already-built message.
    pub fn with_images(mut self, images: Vec<ImageAttachment>) -> Self {
        self.images = images;
        self
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls,
            ..Default::default()
        }
    }

    /// Create an assistant message with reasoning content.
    pub fn assistant_with_reasoning(
        content: impl Into<String>,
        reasoning: Option<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            reasoning_content: reasoning,
            tool_calls,
            ..Default::default()
        }
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
            name: Some(name.into()),
            images: Vec::new(),
            reasoning_content: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_labels_itself_with_its_source_and_size() {
        let image = ImageAttachment {
            media_type: "image/png".into(),
            data: "AAAA".into(),
            bytes: 3,
            name: Some("assets/logo.png".into()),
        };
        assert_eq!(image.label(), "assets/logo.png — image/png, 3 bytes");
        assert_eq!(image.data_url(), "data:image/png;base64,AAAA");
        let unnamed = ImageAttachment {
            data: "AAAA".into(),
            media_type: "image/png".into(),
            ..Default::default()
        };
        assert!(unnamed.label().starts_with("image — "));
    }

    #[test]
    fn messages_without_images_do_not_serialise_the_field() {
        let text = serde_json::to_string(&Message::user("hi")).unwrap();
        assert!(!text.contains("images"), "{text}");
        let with = Message::user("hi").with_images(vec![ImageAttachment {
            media_type: "image/jpeg".into(),
            data: "/9j/".into(),
            bytes: 3,
            name: None,
        }]);
        let round: Message = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(round.images.len(), 1);
        assert_eq!(round.images[0].media_type, "image/jpeg");
        // Older transcripts have no `images` key at all.
        let legacy: Message =
            serde_json::from_str("{\"role\":\"tool\",\"content\":\"x\"}").unwrap();
        assert!(legacy.images.is_empty());
    }
}
