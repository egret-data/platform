//! Delivery transports. P2 ships the webhook; P3 adds email.

pub mod destination;
pub mod email;
pub mod slack;
pub mod slack_config;
pub mod smtp_config;
pub mod webhook;
