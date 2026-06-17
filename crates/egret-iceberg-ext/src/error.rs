use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct IcebergErrorResponse {
    pub error: ErrorModel,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ErrorModel {
    pub message: String,
    pub r#type: String,
    pub code: u16,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stack: Vec<String>,
}

impl ErrorModel {
    pub fn new(message: impl Into<String>, r#type: impl Into<String>, code: u16) -> Self {
        Self {
            message: message.into(),
            r#type: r#type.into(),
            code,
            stack: vec![],
        }
    }
}
