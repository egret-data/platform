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
    /// Set to show this part INSIDE the HTML body instead of listing it as a
    /// file: the value is the `Content-ID` the body's `<img src="cid:…">`
    /// refers to. Only images can be inlined; a PDF or CSV has no `cid:` form,
    /// so those keep `None` and arrive as attachments.
    pub inline_cid: Option<String>,
}

/// One outgoing message.
pub struct EmailMessage<'a> {
    pub to: &'a [String],
    pub subject: &'a str,
    pub body: String,
    /// Optional HTML alternative. When present the mail is `multipart/alternative`
    /// so a text-only client still gets `body` — the HTML is an upgrade, never a
    /// replacement, which is why both are always built.
    pub html: Option<String>,
    /// The triggering rows (alerts), the rendered/exported report, or both —
    /// an alert with a chart attached sends its numbers AND its picture.
    pub attachments: Vec<Attachment>,
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

    // `alternative` when there is HTML: same message, two renderings, and the
    // client picks. Nesting it inside `mixed` below keeps attachments working.
    let alternative = match &msg.html {
        None => MultiPart::mixed().singlepart(text),
        Some(html) => MultiPart::mixed().multipart(
            MultiPart::alternative().singlepart(text).singlepart(
                SinglePart::builder()
                    .header(ContentType::TEXT_HTML)
                    .body(html.clone()),
            ),
        ),
    };

    let parse_type = |a: &Attachment| {
        a.content_type
            .parse::<ContentType>()
            .map_err(|_| EmailError::Build(format!("invalid content type: {}", a.content_type)))
    };

    // An inlined image belongs in `multipart/related` WITH the body that
    // references it, not in `mixed` beside it. Clients resolve `cid:` only
    // within the related part; put the image in `mixed` and it shows as a
    // paperclip while the body renders a broken-image box.
    //
    // The wrapper is added only when something is actually inlined, so a mail
    // with no image keeps byte-for-byte the structure it had before.
    let (inline, files): (Vec<&Attachment>, Vec<&Attachment>) =
        msg.attachments.iter().partition(|a| a.inline_cid.is_some());

    let body_part = if inline.is_empty() {
        alternative
    } else {
        let mut related = MultiPart::related().multipart(alternative);
        for a in &inline {
            let cid = a.inline_cid.clone().expect("partitioned on is_some");
            related = related.singlepart(
                LettreAttachment::new_inline(cid).body(a.bytes.clone(), parse_type(a)?),
            );
        }
        MultiPart::mixed().multipart(related)
    };

    let mut mixed = body_part;
    for a in &files {
        mixed = mixed.singlepart(
            LettreAttachment::new(a.filename.clone()).body(a.bytes.clone(), parse_type(a)?),
        );
    }
    builder
        .multipart(mixed)
        .map_err(|e| EmailError::Build(e.to_string()))
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
            html: None,
            attachments: Vec::new(),
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
            html: None,
            attachments: Vec::new(),
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
            html: None,
            attachments: vec![Attachment {
                filename: "orders.csv".into(),
                content_type: "text/csv".into(),
                bytes: b"\"n\"\n\"7\"\n".to_vec(),
                inline_cid: None,
            }],
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
            html: None,
            attachments: Vec::new(),
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

    fn png(cid: Option<&str>) -> Attachment {
        Attachment {
            filename: "chart.png".into(),
            content_type: "image/png".into(),
            bytes: b"\x89PNG\r\n\x1a\n".to_vec(),
            inline_cid: cid.map(str::to_string),
        }
    }

    /// The structural requirement behind "show the picture in the body": a
    /// `cid:` reference resolves only against parts in the SAME
    /// `multipart/related`. In `mixed` — where every attachment used to go —
    /// the client shows a paperclip and the body renders a broken image.
    #[test]
    fn an_inlined_image_sits_in_multipart_related_with_the_body() {
        let msg = EmailMessage {
            to: &["x@example.com".to_string()],
            subject: "s",
            body: "text".into(),
            html: Some("<img src=\"cid:egret-render\">".into()),
            attachments: vec![png(Some("egret-render"))],
        };
        let raw =
            String::from_utf8(build(&config("none", false), &msg).unwrap().formatted()).unwrap();

        assert!(raw.contains("multipart/related"), "{raw}");
        assert!(raw.contains("Content-ID: <egret-render>"), "{raw}");
        assert!(
            raw.contains("Content-Disposition: inline"),
            "an inline part must not be offered as a download: {raw}"
        );
        // The related wrapper has to CONTAIN the body, not sit beside it.
        let related = raw.find("multipart/related").expect("related part");
        let alternative = raw.find("multipart/alternative").expect("body part");
        assert!(
            related < alternative,
            "the body must be nested inside the related part: {raw}"
        );
    }

    /// A PDF or CSV cannot be inlined, and must keep arriving as a file — so
    /// the related wrapper appears only when something actually uses it.
    #[test]
    fn a_plain_attachment_keeps_the_structure_it_had_before() {
        let msg = EmailMessage {
            to: &["x@example.com".to_string()],
            subject: "s",
            body: "text".into(),
            html: Some("<p>hi</p>".into()),
            attachments: vec![png(None)],
        };
        let raw =
            String::from_utf8(build(&config("none", false), &msg).unwrap().formatted()).unwrap();
        assert!(!raw.contains("multipart/related"), "{raw}");
        assert!(raw.contains("Content-Disposition: attachment"), "{raw}");
    }

    /// Both at once: the render is shown, the numbers are still downloadable.
    #[test]
    fn an_inline_image_and_a_file_can_ride_in_the_same_mail() {
        let msg = EmailMessage {
            to: &["x@example.com".to_string()],
            subject: "s",
            body: "text".into(),
            html: Some("<img src=\"cid:egret-render\">".into()),
            attachments: vec![
                Attachment {
                    filename: "rows.csv".into(),
                    content_type: "text/csv".into(),
                    bytes: b"\"a\"\n".to_vec(),
                    inline_cid: None,
                },
                png(Some("egret-render")),
            ],
        };
        let raw =
            String::from_utf8(build(&config("none", false), &msg).unwrap().formatted()).unwrap();
        assert!(raw.contains("Content-ID: <egret-render>"), "{raw}");
        assert!(raw.contains("rows.csv"), "{raw}");
        assert!(raw.contains("Content-Disposition: attachment"), "{raw}");
    }

    /// The layout the user asked for, asserted as ORDER rather than presence:
    /// "why you are getting this" / picture / result / "what to do about it".
    /// Three `contains` would pass with the footer above the header.
    #[test]
    fn the_image_sits_between_the_opening_and_closing_notes() {
        let html = notification_html(&NotificationBody {
            title: "Weekly orders",
            header: Some("Numbers for Monday's standup."),
            footer: Some("Questions to #data."),
            summary: "The report is below.",
            image_cid: Some("egret-render"),
            url: "https://egret.example/x",
        });
        let at = |needle: &str| {
            html.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {html}"))
        };
        assert!(at("Numbers for Monday") < at("cid:egret-render"));
        assert!(at("cid:egret-render") < at("The report is below."));
        assert!(at("The report is below.") < at("Questions to #data."));
        assert!(at("Questions to #data.") < at("Open in egret"));
    }

    /// A schedule with neither note renders neither paragraph — an empty
    /// footer must not leave a gap where a sentence should be.
    #[test]
    fn absent_notes_render_nothing_at_all() {
        let html = notification_html(&NotificationBody {
            title: "t",
            header: None,
            footer: Some("   "),
            summary: "s",
            image_cid: None,
            url: "u",
        });
        assert_eq!(html.matches("font-size:15px").count(), 0, "{html}");
        assert!(!html.contains("<img"), "{html}");
    }

    /// A note is authored by a workspace editor but read by anyone with an
    /// inbox, so it is escaped — including the footer, which is the new path.
    #[test]
    fn a_note_is_escaped_rather_than_interpolated_as_markup() {
        let html = notification_html(&NotificationBody {
            title: "t",
            header: Some("<script>alert(1)</script>"),
            footer: Some("<b>bold</b>"),
            summary: "s",
            image_cid: None,
            url: "u",
        });
        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("<b>bold</b>"), "{html}");
        assert!(html.contains("&lt;b&gt;bold&lt;/b&gt;"), "{html}");
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
            html: None,
            attachments: Vec::new(),
        };
        assert!(send(&config("none", false), None, &msg).await.is_ok());
    }
}

/// What [`notification_html`] renders around the delivered object.
///
/// A struct rather than five positional arguments: `header`, `footer` and
/// `summary` are all `Option<&str>`-ish text, and at a call site
/// `(&s.name, s.message, s.footer, &body, cid, &url)` is six chances to swap
/// two of them silently.
#[derive(Default)]
pub struct NotificationBody<'a> {
    pub title: &'a str,
    /// The opening note, above everything.
    pub header: Option<&'a str>,
    /// The closing note, below everything the run produced.
    pub footer: Option<&'a str>,
    /// The machine's own line — the value that fired, or what was delivered.
    pub summary: &'a str,
    /// `Content-ID` of an image to show between the header and the summary.
    /// Matches an [`Attachment::inline_cid`]; without that part the client
    /// renders a broken image, so the two are set together or not at all.
    pub image_cid: Option<&'a str>,
    pub url: &'a str,
}

/// Wrap a notification in the house HTML, in the order a reader needs it:
/// the author's opening note, the picture, the machine's summary, the author's
/// closing note, then a link back to the object.
///
/// The picture sits BETWEEN the two notes deliberately. "Here is why you get
/// this" / picture / "here is what to do about it" is how the note reads; an
/// attachment at the bottom of the mail puts the subject of both sentences
/// after the sentences.
///
/// Kept deliberately small and inline-styled. Mail clients strip `<style>`
/// blocks and support no cascade worth relying on, so anything richer would
/// render differently in every inbox — and this is read on a phone more often
/// than not.
pub fn notification_html(b: &NotificationBody<'_>) -> String {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    // A user's note is escaped and line-broken, never interpolated as markup:
    // it is authored by a workspace editor, but the recipient may be anyone
    // with an inbox.
    let note = |text: Option<&str>, margin: &str| {
        text.map(str::trim)
            .filter(|m| !m.is_empty())
            .map(|m| {
                format!(
                    "<p style=\"margin:{margin};font-size:15px;line-height:1.5\">{}</p>",
                    esc(m).replace('\n', "<br>")
                )
            })
            .unwrap_or_default()
    };

    // `max-width:100%` and no height: the render is 2400px wide, and a phone
    // that honours the attribute instead would show a slice of a chart.
    let image = b
        .image_cid
        .map(|cid| {
            format!(
                "<img src=\"cid:{}\" alt=\"{}\" style=\"display:block;width:100%;\
                   max-width:100%;height:auto;margin:0 0 20px;border:1px solid #dce5e6;\
                   border-radius:4px\">",
                esc(cid),
                esc(b.title)
            )
        })
        .unwrap_or_default();

    format!(
        "<html><body style=\"margin:0;padding:24px;background:#f6f8f8;\
           font-family:-apple-system,Segoe UI,Roboto,sans-serif;color:#0f1c1f\">\
         <div style=\"max-width:640px;margin:0 auto;background:#fff;border:1px solid #dce5e6;\
           border-radius:6px;padding:24px\">\
         <h1 style=\"margin:0 0 16px;font-size:18px;font-weight:600\">{}</h1>\
         {}{}\
         <p style=\"margin:0 0 20px;font-size:14px;line-height:1.6;color:#5b6e72\">{}</p>\
         {}\
         <a href=\"{}\" style=\"display:inline-block;background:#0e7c86;color:#fff;\
           text-decoration:none;padding:9px 16px;border-radius:4px;font-size:14px\">Open in egret</a>\
         </div></body></html>",
        esc(b.title),
        note(b.header, "0 0 16px"),
        image,
        esc(b.summary).replace('\n', "<br>"),
        note(b.footer, "0 0 20px"),
        esc(b.url)
    )
}
