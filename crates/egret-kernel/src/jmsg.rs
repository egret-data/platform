//! Minimal Jupyter Messaging Protocol structs for the recorder tap. The Kernel
//! Gateway `/channels` WS multiplexes each message as a JSON object with a
//! `channel` field. We only deserialize the fields the recorder needs; unknown
//! fields are ignored. Forwarding stays byte-verbatim in the proxy.
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct JHeader {
    #[serde(default)]
    pub msg_id: String,
    #[serde(default)]
    pub msg_type: String,
}

#[derive(Debug, Deserialize)]
pub struct JMsg {
    #[serde(default)]
    pub header: JHeader,
    #[serde(default)]
    pub parent_header: JHeader,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub content: serde_json::Value,
}

impl JMsg {
    /// Parse a channels-WS frame; None if it isn't JSON we can read.
    pub fn parse(bytes: &[u8]) -> Option<JMsg> {
        serde_json::from_slice(bytes).ok()
    }
}
