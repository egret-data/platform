//! Auth domain types.

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
