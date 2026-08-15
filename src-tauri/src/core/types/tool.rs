use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn parse_arguments(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_str(&self.arguments)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Function,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: ToolType,
    pub function: FunctionTool,
}

impl ToolDefinition {
    pub fn function(
        name: impl Into<String>,
        description: Option<impl Into<String>>,
        parameters: Value,
    ) -> Self {
        Self {
            kind: ToolType::Function,
            function: FunctionTool {
                name: name.into(),
                description: description.map(Into::into),
                parameters,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Value,
}
