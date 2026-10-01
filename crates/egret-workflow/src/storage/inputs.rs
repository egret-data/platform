//! FILE inputs (W9a spec §7.1).
//!
//! Two ways a `FILE` input gets its value, and one rule for each:
//!
//! - **Uploaded** -- a `multipart/form-data` part with a file name on
//!   `POST .../flows/{id}/executions`. [`upload_input`] streams it to
//!   `inputs/{input}/{file}` of the execution being created, checking the
//!   extension before a byte is stored and the size cap while streaming, and
//!   the input's value is that URI.
//! - **Given as a string** -- JSON on the start route, a `Subflow`'s or a
//!   `Flow` trigger's rendered `inputs:`, or a loop iteration's or replay's
//!   copied inputs. [`check_file_inputs`] requires a `kestra://` URI that
//!   [`super::resolve_readable_many`] -- **the** readability decision, not a
//!   second one -- answers readable from the execution being created. It runs
//!   at every execution-creating INSERT, after the row exists (the readable
//!   set walks from it) and in that INSERT's transaction.
//!
//! [`extension_allowed`] is the one `allowedFileExtensions` check; both paths
//! reach it.

use std::collections::BTreeSet;

use egret_core::Result;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{uri, FileStore, StoredFile};
use crate::models::{Flow, Input, InputType};

/// Why an upload was not stored. The brief's signature returned
/// `Result<_, String>`; that cannot say whose error it is, and the route
/// must (spec §11: a store outage is not the caller's fault), so -- as
/// `collect::CollectError` did for Task 6 -- this says.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    /// The caller's: not a FILE input, an extension `allowedFileExtensions`
    /// refuses, a file name no storage URI can hold. A 4xx naming the input.
    #[error("{0}")]
    Author(String),
    /// The file is over `workflow.storage.max_file_bytes`; the message names
    /// the cap. A 413.
    #[error("{0}")]
    TooLarge(String),
    /// The request body failed while it was being read (the client went away,
    /// a malformed part). The `io::Error` is the body's own, so the route can
    /// recover the multipart parser's status from it.
    #[error("{message}")]
    Body {
        message: String,
        source: std::io::Error,
    },
    /// Internal storage failed. Not the caller's: a 503.
    #[error("{0}")]
    Store(String),
}

/// `allowedFileExtensions` (Kestra's `FileInput.validate`): the extension of
/// `file_name` -- from its last `.`, with the dot, `""` when there is none --
/// must be one of the input's, compared lower-cased on both sides. No list,
/// or an empty one, allows everything. The message is Kestra's
/// (`FileInput.java:47`), prefixed with the input it is about.
///
/// Kestra lower-cases only the file's side, so a declared `.TXT` matches
/// nothing there; egret lower-cases both, which is what the declaration
/// means.
pub fn extension_allowed(input: &Input, file_name: &str) -> std::result::Result<(), String> {
    let Some(allowed) = input.allowed_file_extensions.as_ref() else {
        return Ok(());
    };
    if allowed.is_empty() {
        return Ok(());
    }
    let extension = file_name
        .rfind('.')
        .map(|i| file_name[i..].to_lowercase())
        .unwrap_or_default();
    if allowed.iter().any(|a| a.to_lowercase() == extension) {
        return Ok(());
    }
    Err(format!(
        "Invalid value for input `{}`: {file_name:?}: File type not allowed. Accepted extensions: {}",
        input.id,
        allowed.join(", ")
    ))
}

/// Stream one uploaded part to `inputs/{input}/{file}` of `execution`; the
/// extension is checked before anything is stored and the per-file cap
/// (`workflow.storage.max_file_bytes`) while streaming, so an over-cap file
/// aborts its upload rather than being stored and then refused.
///
/// The file name is kept verbatim (`a%20b.txt` stays `a%20b.txt`, measured on
/// the artifact); one [`uri::input_uri`] cannot read back -- `\`, `?`, `#`, a
/// control character, a `.`/`..` segment -- is the caller's error naming the
/// input and the name.
#[allow(clippy::too_many_arguments)]
pub async fn upload_input(
    fs: &FileStore,
    workspace: Uuid,
    namespace: &str,
    flow_id: &str,
    execution: Uuid,
    input: &Input,
    file_name: &str,
    body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
) -> std::result::Result<StoredFile, UploadError> {
    if input.r#type != InputType::File {
        return Err(UploadError::Author(format!(
            "Invalid value for input `{}`: a file was uploaded for it, and it is not a FILE input",
            input.id
        )));
    }
    if file_name.is_empty() {
        return Err(UploadError::Author(format!(
            "Invalid value for input `{}`: the uploaded file has an empty file name",
            input.id
        )));
    }
    extension_allowed(input, file_name).map_err(UploadError::Author)?;
    let uri = uri::input_uri(namespace, flow_id, execution, &input.id, file_name)
        .map_err(|e| UploadError::Author(format!("Invalid value for input `{}`: {e}", input.id)))?;
    let object = fs
        .object_uri(workspace, &uri)
        .map_err(|e| UploadError::Author(format!("Invalid value for input `{}`: {e}", input.id)))?;
    let cap = fs.caps.max_file_bytes;
    let mut guarded = Guarded {
        inner: body,
        cap,
        read: 0,
        over: false,
        failed: None,
    };
    let put = fs.store.put_stream(&object, &mut guarded, None).await;
    if guarded.over {
        return Err(UploadError::TooLarge(too_large(&input.id, cap)));
    }
    if let Some(source) = guarded.failed {
        return Err(UploadError::Body {
            message: format!(
                "Invalid value for input `{}`: the upload of {file_name:?} did not complete: {source}",
                input.id
            ),
            source,
        });
    }
    let written = put.map_err(|e| {
        UploadError::Store(format!(
            "{uri} could not be stored: internal storage is unavailable: {e}"
        ))
    })?;
    let size = match fs.store.head(&object).await {
        Ok(Some(size)) => size,
        Ok(None) => {
            return Err(UploadError::Store(format!(
                "{uri} was written ({written} bytes) but internal storage does not have it"
            )))
        }
        Err(e) => {
            return Err(UploadError::Store(format!(
                "{uri} could not be checked: internal storage is unavailable: {e}"
            )))
        }
    };
    Ok(StoredFile { uri, size })
}

/// The refusal for a file over `cap`, naming the input and the cap. Shared
/// with the route, whose body limit can cut a part off before this reader
/// sees the byte that crosses the cap.
pub fn too_large(input: &str, cap: u64) -> String {
    format!(
        "Invalid value for input `{input}`: the file is over the {cap} bytes one file may be \
         (workflow.storage.max_file_bytes)"
    )
}

/// The uploaded body as `put_stream` reads it: counts bytes, refuses the one
/// that crosses the cap, and keeps the body's own error so it is told apart
/// from the store's (which `put_stream` reports as its own).
struct Guarded<'a> {
    inner: &'a mut (dyn tokio::io::AsyncRead + Send + Unpin),
    cap: u64,
    read: u64,
    over: bool,
    failed: Option<std::io::Error>,
}

impl tokio::io::AsyncRead for Guarded<'_> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        let this = self.get_mut();
        let before = buf.filled().len();
        match std::pin::Pin::new(&mut *this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                this.read += (buf.filled().len() - before) as u64;
                if this.read > this.cap {
                    this.over = true;
                    // `AsyncRead`'s contract: an `Err` fills nothing (tokio's
                    // `read_to_end` asserts it), so the bytes just read are
                    // handed back unread.
                    buf.set_filled(before);
                    return Poll::Ready(Err(std::io::Error::other(
                        "the file is over workflow.storage.max_file_bytes",
                    )));
                }
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(e)) => {
                let message = e.to_string();
                this.failed.get_or_insert(e);
                Poll::Ready(Err(std::io::Error::other(message)))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Every FILE input's value must be a `kestra://` URI readable from
/// `execution` ([`super::resolve_readable_many`] over
/// [`super::readable_executions`]) whose file name `allowedFileExtensions`
/// accepts. Called at every execution-creating INSERT, after the row exists.
///
/// An absent or `null` value is not checked (a default, or no value; the
/// render decides). `uploaded` are the files [`upload_input`] stored for
/// THIS execution, indexed in the same transaction after this check: a value
/// equal to one of them is not looked up (its row is not written yet); every
/// other value is.
///
/// `Ok(Err(message))` is the refusal, naming the input -- the caller's error
/// at the start route, a failed `Subflow` run at a spawn. `Err` is the
/// database's.
pub async fn check_file_inputs(
    conn: &mut PgConnection,
    workspace: Uuid,
    execution: Uuid,
    flow: &Flow,
    inputs: &serde_json::Value,
    uploaded: &[StoredFile],
) -> Result<std::result::Result<(), String>> {
    let uploaded: BTreeSet<&str> = uploaded.iter().map(|f| f.uri.as_str()).collect();
    let mut given: Vec<(&Input, String)> = Vec::new();
    for input in flow.inputs.iter().filter(|i| i.r#type == InputType::File) {
        match inputs.get(&input.id) {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::String(s)) => given.push((input, s.clone())),
            Some(other) => {
                return Ok(Err(format!(
                    "Invalid value for input `{}`: a FILE input is a kestra:// URI string, not {}",
                    input.id,
                    kind_of(other)
                )))
            }
        }
    }
    if given.is_empty() {
        return Ok(Ok(()));
    }
    let lookup: Vec<String> = given
        .iter()
        .filter(|(_, v)| !uploaded.contains(v.as_str()))
        .map(|(_, v)| v.clone())
        .collect();
    let resolved = if lookup.is_empty() {
        Default::default()
    } else {
        let readable = super::readable_executions(conn, workspace, execution).await?;
        super::resolve_readable_many(conn, workspace, &readable, &lookup).await?
    };
    for (input, value) in &given {
        if !uploaded.contains(value.as_str()) {
            match resolved.get(value) {
                Some(Ok(_)) => {}
                Some(Err(why)) => {
                    return Ok(Err(format!(
                        "Invalid value for input `{}`: {why}",
                        input.id
                    )))
                }
                None => {
                    return Err(egret_core::Error::Internal(anyhow::anyhow!(
                        "resolve_readable_many did not answer {value}"
                    )))
                }
            }
        }
        // Readable (or uploaded), so it parses: the file name is the URI's
        // last segment.
        let file_name = value.rsplit('/').next().unwrap_or_default();
        if let Err(why) = extension_allowed(input, file_name) {
            return Ok(Err(why));
        }
    }
    Ok(Ok(()))
}

fn kind_of(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Null => "null",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn file_input(allowed: Option<&[&str]>) -> Input {
        let mut yaml = "id: f\ntype: FILE\n".to_string();
        if let Some(a) = allowed {
            yaml.push_str(&format!(
                "allowedFileExtensions: [{}]\n",
                a.iter()
                    .map(|e| format!("{e:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        serde_norway::from_str(&yaml).unwrap()
    }

    fn oracle() -> serde_json::Value {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/w9a-storage.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// `w9a-storage.json` `file_input.allowed_file_extensions` (basis
    /// `chosen`): `[".txt"]` refuses `bad.csv` with Kestra's message.
    #[test]
    fn the_extension_refusal_is_the_artifacts_message_naming_the_input() {
        let cell = &oracle()["egret_expected"]["file_input"]["allowed_file_extensions"];
        assert_eq!(cell["refused"], true);
        let declared: Vec<&str> = cell["flow_declares"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let input = file_input(Some(&declared));
        let err = extension_allowed(&input, "bad.csv").unwrap_err();
        assert!(err.contains(cell["message"].as_str().unwrap()), "{err}");
        assert!(err.contains("input `f`"), "{err}");
        // The accepting half, and both sides lower-cased.
        assert_eq!(extension_allowed(&input, "hello.txt"), Ok(()));
        assert_eq!(extension_allowed(&input, "UP.TXT"), Ok(()));
        assert_eq!(
            extension_allowed(&file_input(Some(&[".TXT"])), "a.txt"),
            Ok(())
        );
        // No extension is "", which `.txt` is not.
        assert!(extension_allowed(&input, "noext").is_err());
        // The LAST dot.
        assert!(extension_allowed(&input, "a.txt.csv").is_err());
        assert_eq!(extension_allowed(&input, "a.csv.txt"), Ok(()));
        // No list, or an empty one, allows anything.
        assert_eq!(extension_allowed(&file_input(None), "bad.csv"), Ok(()));
        assert_eq!(extension_allowed(&file_input(Some(&[])), "bad.csv"), Ok(()));
    }

    fn store(max_file_bytes: u64) -> FileStore {
        FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps: super::super::Caps {
                max_file_bytes,
                ..Default::default()
            },
            retention_days: 30,
        }
    }

    /// The URI is `file_input.uri_tail`'s layout, the bytes land at it, and
    /// the size is the stored one. Exactly at the cap is allowed; one byte
    /// over is refused naming the cap, and nothing is left stored.
    #[tokio::test]
    async fn an_upload_lands_at_inputs_input_file_and_the_cap_is_enforced_while_streaming() {
        let fs = store(5);
        let ws = Uuid::new_v4();
        let ex = Uuid::new_v4();
        let input = file_input(None);
        let tail = oracle()["egret_expected"]["file_input"]["uri_tail"]
            .as_str()
            .unwrap()
            .to_string();
        let file = tail.rsplit('/').next().unwrap();
        let got = upload_input(
            &fs,
            ws,
            "w9a.storage",
            "file-in",
            ex,
            &input,
            file,
            &mut &b"hello"[..],
        )
        .await
        .expect("at the cap");
        assert!(
            got.uri.ends_with(&format!("/executions/{ex}/{tail}")),
            "{}",
            got.uri
        );
        assert_eq!(got.size, 5);
        let object = fs.object_uri(ws, &got.uri).unwrap();
        assert_eq!(fs.store.get(&object).await.unwrap(), b"hello");

        let err = upload_input(
            &fs,
            ws,
            "w9a.storage",
            "file-in",
            ex,
            &input,
            "big.txt",
            &mut &b"hello!"[..],
        )
        .await
        .expect_err("one byte over");
        assert!(
            matches!(&err, UploadError::TooLarge(m) if m.contains("5 bytes") && m.contains("max_file_bytes") && m.contains("`f`")),
            "{err:?}"
        );
        let big = fs
            .object_uri(
                ws,
                &uri::input_uri("w9a.storage", "file-in", ex, "f", "big.txt").unwrap(),
            )
            .unwrap();
        assert_eq!(fs.store.head(&big).await.unwrap(), None, "nothing stored");
    }

    /// File names: kept verbatim where Kestra keeps them
    /// (`file_input.filenames`: `%20`, `+`, accents); one no URI can hold is
    /// the caller's error naming the input, never a store call.
    #[tokio::test]
    async fn file_names_are_kept_verbatim_and_an_unholdable_one_is_the_callers_error() {
        let fs = store(1024);
        let ws = Uuid::new_v4();
        let ex = Uuid::new_v4();
        let input = file_input(None);
        let cells = &oracle()["egret_expected"]["file_input"]["filenames"];
        for key in ["percent20", "plus", "accent", "uppercase_extension"] {
            let want = cells[key]["inputs_f"].as_str().unwrap();
            let name = want.rsplit('/').next().unwrap();
            let got = upload_input(
                &fs,
                ws,
                "w9a.storage",
                "file-in",
                ex,
                &input,
                name,
                &mut &b"x"[..],
            )
            .await
            .unwrap_or_else(|e| panic!("{key}: {e}"));
            assert!(
                got.uri.ends_with(&format!("/inputs/f/{name}")),
                "{key}: {}",
                got.uri
            );
        }
        for bad in ["a?b.txt", "a#b.txt", "a\\b.txt", "a\nb.txt", "..", "."] {
            let err = upload_input(
                &fs,
                ws,
                "w9a.storage",
                "file-in",
                ex,
                &input,
                bad,
                &mut &b"x"[..],
            )
            .await
            .expect_err(bad);
            assert!(
                matches!(&err, UploadError::Author(m) if m.contains("input `f`")),
                "{bad:?}: {err:?}"
            );
        }
        let err = upload_input(
            &fs,
            ws,
            "w9a.storage",
            "file-in",
            ex,
            &input,
            "",
            &mut &b"x"[..],
        )
        .await
        .expect_err("empty");
        assert!(
            matches!(&err, UploadError::Author(m) if m.contains("empty file name")),
            "{err:?}"
        );
    }

    /// A disallowed extension is refused before anything is stored, and a
    /// file for a non-FILE input is refused.
    #[tokio::test]
    async fn a_refused_extension_or_a_non_file_input_stores_nothing() {
        let fs = store(1024);
        let ws = Uuid::new_v4();
        let ex = Uuid::new_v4();
        let err = upload_input(
            &fs,
            ws,
            "w9a.storage",
            "file-in",
            ex,
            &file_input(Some(&[".txt"])),
            "bad.csv",
            &mut &b"x"[..],
        )
        .await
        .expect_err("csv");
        assert!(
            matches!(&err, UploadError::Author(m) if m.contains("File type not allowed")),
            "{err:?}"
        );
        let text: Input = serde_norway::from_str("id: t\ntype: STRING\n").unwrap();
        let err = upload_input(
            &fs,
            ws,
            "w9a.storage",
            "file-in",
            ex,
            &text,
            "a.txt",
            &mut &b"x"[..],
        )
        .await
        .expect_err("STRING");
        assert!(
            matches!(&err, UploadError::Author(m) if m.contains("not a FILE input")),
            "{err:?}"
        );
        let listed = fs.store.list("workflow/", None).await.unwrap();
        assert!(listed.objects.is_empty(), "{:?}", listed.objects);
    }
}
