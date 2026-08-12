//! Slack delivery: post a message to one channel via `chat.postMessage`.
//!
//! Deliberately NOT built on `webhook.rs`. An incoming-webhook URL is itself
//! the credential, so it would sit in the recipient config where anyone who can
//! read the schedule could lift it and post as the workspace. A bot token lives
//! in the vault exactly like the webhook signing secret (see
//! `ScheduleRepo::vault_secrets`), and the config keeps only a channel name and
//! a reference.
//!
//! No SSRF vetting here, and none needed: the destination is not user-supplied.
//! It is always `slack.com`, unlike a webhook, where the whole point of
//! `destination::vet` is that the user chose the host.

use std::time::Duration;

/// Slack's own endpoint. A constant, not configuration — the moment this is
/// settable it becomes an exfiltration path for the bot token.
const POST_MESSAGE_URL: &str = "https://slack.com/api/chat.postMessage";

/// Same cap as a webhook: a slow API must not hold a scheduler slot.
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum SlackError {
    #[error("slack request failed: {0}")]
    Transport(String),
    #[error("slack returned {0}")]
    Status(u16),
    /// Slack answers 200 with `{"ok": false, "error": "..."}` for most real
    /// failures — an invalid token, a channel the bot is not in. Treating that
    /// as success is how a report silently stops arriving.
    #[error("slack rejected the message: {0}")]
    Rejected(String),
    #[error("no slack bot token is configured for this destination")]
    NoToken,
}

/// Post `text` to `channel`.
///
/// `text` is Slack's plain-text field, which it renders with its own light
/// markup. The attachment story is deliberately absent: uploading a file needs
/// a second, differently-scoped API call, and a link back to egret (which the
/// message carries) is more useful to a reader than a PNG in a thread.
pub async fn deliver(token: &str, channel: &str, text: &str) -> Result<(), SlackError> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|e| SlackError::Transport(e.to_string()))?;

    let resp = client
        .post(POST_MESSAGE_URL)
        .bearer_auth(token)
        .json(&serde_json::json!({ "channel": channel, "text": text }))
        .send()
        .await
        .map_err(|e| SlackError::Transport(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(SlackError::Status(status.as_u16()));
    }

    // The body decides, not the status line.
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| SlackError::Transport(e.to_string()))?;
    if body.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    Err(SlackError::Rejected(
        body.get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error")
            .to_string(),
    ))
}

/// The message body for one run: the user's note, the summary, and a link.
///
/// Slack renders `<url|label>` as a link; the label is the only place the
/// object's name appears, so a message with no name reads as spam.
pub fn message(name: &str, note: Option<&str>, summary: &str, url: &str) -> String {
    let mut out = String::new();
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        out.push_str(n);
        out.push_str("\n\n");
    }
    out.push_str(summary.trim());
    out.push_str(&format!("\n\n<{url}|Open \"{name}\" in egret>"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_carries_the_note_the_summary_and_a_named_link() {
        let m = message(
            "Orders below target",
            Some("Check the EU region first."),
            "Observed value: 412",
            "https://egret.test/explore/demo/alerts",
        );
        assert!(m.starts_with("Check the EU region first."), "{m}");
        assert!(m.contains("Observed value: 412"));
        // The link must be Slack's own syntax, or it renders as raw text.
        assert!(
            m.contains(
                "<https://egret.test/explore/demo/alerts|Open \"Orders below target\" in egret>"
            ),
            "{m}"
        );
    }

    #[test]
    fn an_absent_note_leaves_no_blank_lines_at_the_top() {
        let m = message("Weekly", None, "The report is attached.", "https://x/y");
        assert!(m.starts_with("The report is attached."), "{m}");
    }
}
