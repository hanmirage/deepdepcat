use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ConversationItem {
    System(SystemMessage),
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    Reasoning(ReasoningMessage),
}

impl ConversationItem {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System(SystemMessage {
            content: content.into(),
        })
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User(UserMessage {
            content: vec![ContentPart::Text {
                text: content.into(),
            }],
        })
    }

    pub fn user_with_parts(parts: Vec<ContentPart>) -> Self {
        Self::User(UserMessage { content: parts })
    }

    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: false,
        })
    }

    pub fn tool_result_error(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.into(),
            content: content.into(),
            is_error: true,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMessage {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: String,
    pub tool_calls: Vec<super::tool::ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<super::info::TokenUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub content: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningMessage {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        #[serde(rename = "source_type", default = "default_image_source")]
        source_type: String,
        #[serde(rename = "media_type")]
        media_type: String,
        data: String,
    },
}

fn default_image_source() -> String {
    "base64".to_string()
}
