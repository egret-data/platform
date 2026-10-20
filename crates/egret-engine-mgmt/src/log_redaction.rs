//! Redaction of credential-shaped `key=value` pairs from engine pod logs
//! before egret serves them (`GET /engines/{id}/pods/{pod}/logs` and its
//! `/follow` stream, both `engine:read`).
//!
//! Defence in depth. The source of the one known leak, the Trino Ranger
//! plugin's `AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.password=…`
//! line, is silenced in the rendered `log.properties`
//! (`egret_provisioner::trino`), but an engine still running the old config,
//! a user log level egret does not override, or another component's log can
//! print a secret the same way. `kubectl logs` is not covered: this only
//! stops egret from handing the value to an `engine:read` caller.
//!
//! Conservative on purpose: only a key that contains `password`, `passwd` or
//! `secret` followed by `=` or `:` has its value (the rest of the
//! whitespace-free token) replaced by `***`; an already-masked `****` becomes
//! `***`. `token` is not matched, because
//! it would also hide query text and counters that are not secret.

use bytes::Bytes;
use futures_util::StreamExt;

const PATTERN: &str =
    r"(?i)(\b[A-Za-z0-9_.\-]*(?:password|passwd|secret)[A-Za-z0-9_.\-]*[ \t]*[=:][ \t]*)\S+";

/// `anyhow::Result<Bytes>`, named through the stream so this crate needs no
/// `anyhow` of its own.
type Item = <egret_provisioner::kube_client::PodLogStream as futures_util::Stream>::Item;

const MASK: &str = "${1}***";

/// A partial line longer than this is passed on without waiting for its end,
/// so a log without newlines cannot grow the buffer without bound.
const MAX_CARRY: usize = 64 * 1024;

fn text_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(PATTERN).expect("valid redaction regex"))
}

fn bytes_re() -> &'static regex::bytes::Regex {
    static RE: std::sync::OnceLock<regex::bytes::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::bytes::Regex::new(PATTERN).expect("valid redaction regex"))
}

/// Redact a whole log text.
pub fn redact_log_text(logs: &str) -> String {
    text_re().replace_all(logs, MASK).into_owned()
}

fn redact_bytes(chunk: &[u8]) -> Bytes {
    Bytes::from(bytes_re().replace_all(chunk, MASK.as_bytes()).into_owned())
}

/// Re-chunks a log byte stream on line ends and redacts each complete line,
/// so a value split across two chunks is still found. The tail without a
/// newline is held until the next newline, the end of the stream, or
/// [`MAX_CARRY`]. An error item passes through after what was held.
pub fn redact_log_stream(
    inner: egret_provisioner::kube_client::PodLogStream,
) -> egret_provisioner::kube_client::PodLogStream {
    struct State {
        inner: egret_provisioner::kube_client::PodLogStream,
        carry: Vec<u8>,
        done: bool,
        pending_err: Option<Item>,
    }
    let state = State {
        inner,
        carry: Vec::new(),
        done: false,
        pending_err: None,
    };
    Box::pin(futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if st.done {
                return st.pending_err.take().map(|e| (e, st));
            }
            match st.inner.next().await {
                Some(Ok(chunk)) => {
                    st.carry.extend_from_slice(&chunk);
                    let cut = match st.carry.iter().rposition(|b| *b == b'\n') {
                        Some(i) => i + 1,
                        None if st.carry.len() > MAX_CARRY => st.carry.len(),
                        None => continue,
                    };
                    let ready: Vec<u8> = st.carry.drain(..cut).collect();
                    return Some((Ok(redact_bytes(&ready)), st));
                }
                Some(Err(e)) => {
                    st.done = true;
                    if st.carry.is_empty() {
                        return Some((Err(e), st));
                    }
                    // What was held goes out redacted first; the error after.
                    let ready = std::mem::take(&mut st.carry);
                    st.pending_err = Some(Err(e));
                    return Some((Ok(redact_bytes(&ready)), st));
                }
                None => {
                    st.done = true;
                    if st.carry.is_empty() {
                        return None;
                    }
                    let ready = std::mem::take(&mut st.carry);
                    return Some((Ok(redact_bytes(&ready)), st));
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RANGER_LINE: &str = "2026-10-20T21:12:18.873Z\tINFO\tmain\torg.apache.ranger.audit.provider.AuditProviderFactory\tAUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.password=JWabc-DEF_123xyz\n";

    #[test]
    fn the_ranger_password_property_is_masked_and_the_rest_kept() {
        let out = redact_log_text(RANGER_LINE);
        assert!(!out.contains("JWabc"), "{out}");
        assert!(
            out.ends_with("AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.password=***\n"),
            "{out}"
        );
        assert!(out.starts_with("2026-10-20T21:12:18.873Z\tINFO\tmain\t"));
    }

    #[test]
    fn other_secret_shaped_pairs_are_masked() {
        let out = redact_log_text(
            "a db.password = hunter2 b\nSECRET_KEY:xyz\nexchange.s3.aws-secret-key=AKIA/x+y\n",
        );
        assert_eq!(
            out,
            "a db.password = *** b\nSECRET_KEY:***\nexchange.s3.aws-secret-key=***\n"
        );
    }

    #[test]
    fn ordinary_lines_and_the_username_are_left_alone() {
        let text = "INFO AUDIT PROPERTY: ranger.plugin.trino.policy.rest.client.username=engine-c4c3\nINFO PolicyRefresher(serviceName=live-trino): found updated version. lastKnownVersion=-1; newVersion=35\nquery token=5 tokens\n";
        assert_eq!(redact_log_text(text), text);
    }

    #[test]
    fn the_value_does_not_run_onto_the_next_line() {
        assert_eq!(
            redact_log_text("x.password=\nnext line\n"),
            "x.password=\nnext line\n"
        );
    }

    fn chunks(parts: &[&str]) -> egret_provisioner::kube_client::PodLogStream {
        let items: Vec<Item> = parts
            .iter()
            .map(|p| Ok(Bytes::copy_from_slice(p.as_bytes())))
            .collect();
        Box::pin(futures_util::stream::iter(items))
    }

    async fn collect(s: egret_provisioner::kube_client::PodLogStream) -> String {
        let parts: Vec<Bytes> = s.map(|r| r.expect("ok")).collect().await;
        String::from_utf8(parts.concat()).unwrap()
    }

    #[tokio::test]
    async fn a_password_split_across_stream_chunks_is_still_masked() {
        let (a, b) = RANGER_LINE.split_at(RANGER_LINE.find("JWabc").unwrap() + 3);
        let (b1, b2) = b.split_at(4);
        let out = collect(redact_log_stream(chunks(&["first\n", a, b1, b2, "last"]))).await;
        assert!(!out.contains("JWabc"), "{out}");
        assert_eq!(
            out,
            format!("first\n{}last", redact_log_text(RANGER_LINE)),
            "every byte but the value goes through, the unterminated tail at the end"
        );
    }

    #[tokio::test]
    async fn an_error_comes_after_the_held_tail() {
        let items: Vec<Item> = vec![
            Ok(Bytes::from_static(b"x.password=abc")),
            Err(std::io::Error::other("boom").into()),
        ];
        let out: Vec<Item> = redact_log_stream(Box::pin(futures_util::stream::iter(items)))
            .collect()
            .await;
        assert_eq!(out.len(), 2, "the held tail, then the error");
        assert_eq!(&out[0].as_ref().unwrap()[..], b"x.password=***");
        assert!(out[1].is_err());
    }
}
