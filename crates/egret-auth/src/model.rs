//! Auth domain types.

use chrono::{DateTime, Utc};
use egret_core::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Local,
    Oidc,
}

impl Provider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Oidc => "oidc",
        }
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: UserId,
    pub provider: String, // 'local' | 'oidc'
    pub email: String,
    pub name: String,
    pub password_hash: Option<String>,
    pub status: String, // 'active' | 'disabled'
}

#[derive(Debug, Clone)]
pub struct Session {
    pub id: uuid::Uuid,
    pub user_id: UserId,
    pub absolute_expires_at: DateTime<Utc>,
    pub idle_expires_at: DateTime<Utc>,
}
