//! Email delivery over the workspace's own SMTP relay.
//!
//! The message body is deliberately plain: the value that fired, and the rows
//! that produced it as a CSV attachment. An alert people cannot read on a phone
//! at 3am is an alert they will turn off.

use lettre::message::{header::ContentType, Attachment as LettreAttachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use super::smtp_config::{SmtpConfig, TlsMode};

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("smtp is not configured for this workspace")]
    NotConfigured,
    #[error("invalid address: {0}")]
    Address(String),
    #[error("smtp transport: {0}")]
    Transport(String),
    #[error("could not build the message: {0}")]
    Build(String),
}

/// Render `rows` as CSV, for the attachment.
///
/// Quoting is applied to every field rather than only where required: it costs
/// nothing, and the alternative is deciding per-cell whether a value contains a
/// comma, a quote or a newline — a decision that is wrong exactly once before
/// someone's spreadsheet silently gains a column.
pub fn rows_to_csv(columns: &[String], rows: &serde_json::Value) -> String {
    let mut out = String::new();
    push_row(&mut out, columns.iter().map(String::as_str));

    if let Some(rows) = rows.as_array() {
        for row in rows {
            let cells: Vec<String> = match row.as_array() {
                Some(cells) => cells.iter().map(cell_to_string).collect(),
                None => vec![cell_to_string(row)],
            };
            push_row(&mut out, cells.iter().map(String::as_str));
        }
    }
    out
}

fn cell_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn push_row<'a>(out: &mut String, cells: impl Iterator<Item = &'a str>) {
    let mut first = true;
    for c in cells {
        if !first {
            out.push(',');
        }
        first = false;
        out.push('"');
        out.push_str(&c.replace('"', "\"\""));
        out.push('"');
    }
    out.push('\n');
}

/// A single file riding along with an [`EmailMessage`].
///
/// Generic over content type (rather than a `csv`-shaped field) because P4
/// adds a second attachment shape — a rendered PNG/PDF — and a report's `csv`
/// format still uses this same struct with `content_type: "text/csv"`. Bytes
/// rather than `String`: an image/PDF is binary, and a CSV is ASCII-clean
/// bytes either way, so one field covers both without a second variant.
#[derive(Debug, Clone)]
pub struct Attachment {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// One outgoing message.
pub struct EmailMessage<'a> {
    pub to: &'a [String],
    pub subject: &'a str,
    pub body: String,
    /// The triggering rows (alerts) or the rendered/exported report, when
    /// there is one to send.
    pub attachment: Option<Attachment>,
}

/// Build the `lettre` transport for `config`.
///
/// The TLS mode is taken from the configuration, never inferred from the port.
/// `587 means STARTTLS` is a convention; guessing it wrong means either a
/// failure the operator can see, or — far worse — credentials sent in the clear
/// on a relay that happens to accept them.
pub fn transport(
    config: &SmtpConfig,
    password: Option<String>,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, EmailError> {
    let mode = TlsMode::parse(&config.tls_mode).unwrap_or(TlsMode::Starttls);
    let builder = match mode {
        TlsMode::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
            .map_err(|e| EmailError::Transport(e.to_string()))?,
        TlsMode::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)
            .map_err(|e| EmailError::Transport(e.to_string()))?,
        // Explicitly chosen, for a trusted in-cluster relay or a mail catcher.
        TlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host),
    };
    let builder = builder.port(config.port as u16);
    let builder = match (&config.username, password) {
        (Some(u), Some(p)) => builder.credentials(Credentials::new(u.clone(), p)),
        _ => builder,
    };
    Ok(builder.build())
}

/// Assemble the MIME message.
pub fn build(config: &SmtpConfig, msg: &EmailMessage<'_>) -> Result<Message, EmailError> {
    let from = match &config.from_name {
        Some(n) => format!("{n} <{}>", config.from_address),
        None => config.from_address.clone(),
    };
    let mut builder = Message::builder()
        .from(
            from.parse()
                .map_err(|_| EmailError::Address(from.clone()))?,
        )
        .subject(msg.subject);
    for to in msg.to {
        builder = builder.to(to.parse().map_err(|_| EmailError::Address(to.clone()))?);
    }

    let text = SinglePart::builder()
        .header(ContentType::TEXT_PLAIN)
        .body(msg.body.clone());

    match &msg.attachment {
        None => builder
            .singlepart(text)
            .map_err(|e| EmailError::Build(e.to_string())),
        Some(a) => {
            let content_type = a.content_type.parse::<ContentType>().map_err(|_| {
                EmailError::Build(format!("invalid content type: {}", a.content_type))
            })?;
            let attachment =
                LettreAttachment::new(a.filename.clone()).body(a.bytes.clone(), content_type);
            builder
                .multipart(MultiPart::mixed().singlepart(text).singlepart(attachment))
                .map_err(|e| EmailError::Build(e.to_string()))
        }
    }
}

/// Build and send.
pub async fn send(
    config: &SmtpConfig,
    password: Option<String>,
    msg: &EmailMessage<'_>,
) -> Result<(), EmailError> {
    if msg.to.is_empty() {
        return Ok(());
    }
    let mailer = transport(config, password)?;
    let message = build(config, msg)?;
    mailer
        .send(message)
        .await
        .map(|_| ())
        .map_err(|e| EmailError::Transport(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn config(tls: &str, auth: bool) -> SmtpConfig {
        SmtpConfig {
            workspace_id: Uuid::new_v4(),
            host: "smtp.example.com".into(),
            port: 587,
            username: auth.then(|| "mailer".to_string()),
            from_address: "alerts@example.com".into(),
            from_name: Some("Egret Alerts".into()),
            tls_mode: tls.into(),
            password_secret_id: auth.then(Uuid::new_v4),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn every_tls_mode_builds_a_transport() {
        // Each arm is a different lettre constructor; a mode that fails to
        // build would only surface at send time, which is 3am.
        for mode in ["starttls", "implicit", "none"] {
            assert!(
                transport(&config(mode, true), Some("pw".into())).is_ok(),
                "{mode} must build"
            );
        }
    }

    #[test]
    fn a_relay_with_no_credentials_still_builds() {
        // An unauthenticated in-cluster relay is a normal deployment.
        assert!(transport(&config("none", false), None).is_ok());
    }

    #[test]
    fn the_from_header_uses_the_display_name_when_set() {
        let msg = EmailMessage {
            to: &["on-call@example.com".to_string()],
            subject: "[alert] orders",
            body: "value 7".into(),
            attachment: None,
        };
        let built = build(&config("starttls", false), &msg).unwrap();
        let raw = String::from_utf8(built.formatted()).unwrap();
        assert!(
            raw.contains("From: \"Egret Alerts\" <alerts@example.com>"),
            "expected a named From, got:\n{raw}"
        );
        assert!(raw.contains("To: on-call@example.com"));
        assert!(raw.contains("[alert] orders"));
    }

    #[test]
    fn a_bare_from_address_is_used_when_no_display_name_is_set() {
        let mut c = config("starttls", false);
        c.from_name = None;
        let msg = EmailMessage {
            to: &["x@example.com".to_string()],
            subject: "s",
            body: "b".into(),
            attachment: None,
        };
        let raw = String::from_utf8(build(&c, &msg).unwrap().formatted()).unwrap();
        assert!(raw.contains("From: alerts@example.com"), "{raw}");
    }

    #[test]
    fn the_csv_rides_as_an_attachment_not_as_body_text() {
        // Pasting rows into the body makes the mail unreadable the moment the
        // result is wider than a phone screen.
        let msg = EmailMessage {
            to: &["x@example.com".to_string()],
            subject: "s",
            body: "value 7".into(),
            attachment: Some(Attachment {
                filename: "orders.csv".into(),
                content_type: "text/csv".into(),
                bytes: b"\"n\"\n\"7\"\n".to_vec(),
            }),
        };
        let raw =
            String::from_utf8(build(&config("none", false), &msg).unwrap().formatted()).unwrap();
        assert!(raw.contains("multipart/mixed"), "{raw}");
        assert!(raw.contains("text/csv"));
        assert!(raw.contains("orders.csv"));
    }

    #[test]
    fn an_invalid_recipient_is_refused_rather_than_sent_to() {
        let msg = EmailMessage {
            to: &["not an address".to_string()],
            subject: "s",
            body: "b".into(),
            attachment: None,
        };
        assert!(matches!(
            build(&config("none", false), &msg),
            Err(EmailError::Address(_))
        ));
    }

    #[test]
    fn csv_quotes_every_field_and_escapes_embedded_quotes() {
        // The cases that corrupt a spreadsheet: a comma, a quote, a newline.
        let rows = serde_json::json!([
            ["plain", 7],
            ["has,comma", null],
            ["has\"quote", true],
            ["has\nnewline", 1.5],
        ]);
        let csv = rows_to_csv(&["name".into(), "v".into()], &rows);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], r#""name","v""#);
        assert_eq!(lines[1], r#""plain","7""#);
        assert_eq!(lines[2], r#""has,comma","""#, "null becomes empty");
        assert_eq!(lines[3], r#""has""quote","true""#, "quote is doubled");
        // The embedded newline stays inside its quoted field, so the row count
        // a reader sees still matches the row count that fired.
        assert!(csv.contains("\"has\nnewline\",\"1.5\""), "{csv}");
    }

    #[test]
    fn csv_of_no_rows_is_still_a_valid_header_only_file() {
        let csv = rows_to_csv(&["a".into()], &serde_json::json!([]));
        assert_eq!(csv, "\"a\"\n");
    }

    #[tokio::test]
    async fn sending_to_nobody_is_a_no_op_rather_than_an_error() {
        // An alert whose only recipients are webhooks reaches this path; it
        // must not report a failure it did not have.
        let msg = EmailMessage {
            to: &[],
            subject: "s",
            body: "b".into(),
            attachment: None,
        };
        assert!(send(&config("none", false), None, &msg).await.is_ok());
    }
}
