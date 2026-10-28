//! The Server-Sent Events parser of `http.SseRequest` (package #4 Task C4,
//! spec D8): Kestra's `HttpClient.parseSse` (`HttpClient.java:337-419`),
//! which follows the WHATWG event-stream rules, fed chunk by chunk.
//!
//! What it keeps from Kestra, line by line:
//!
//! - Lines end at `\n`, `\r` or `\r\n` (`BufferedReader.readLine`), and are
//!   decoded as UTF-8 with a malformed sequence becoming U+FFFD
//!   (`InputStreamReader`).
//! - An empty line dispatches the event **only when a `data` field was
//!   seen**, with the data's final `\n` removed; every field resets after an
//!   empty line either way.
//! - A line starting with `:` is a comment, kept (the last one before the
//!   dispatch wins), its single leading space removed.
//! - `field: value` strips ONE leading space from the value; a line with no
//!   colon is a field with an empty value. `data` appends the value and a
//!   `\n`; `id` is ignored when it holds a NUL; `event` is the name; `retry`
//!   is kept only when it is all ASCII digits and fits a `long`. Any other
//!   field is ignored.
//! - The end of the stream does NOT dispatch a pending event.

/// One dispatched event, before its data is read as JSON.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseEvent {
    /// `None` only when a `data` line was seen but every one was empty
    /// (Kestra's `parsedData` stays null when the buffer is empty).
    pub data: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub comment: Option<String>,
    pub retry_ms: Option<u64>,
}

/// The parser's state between chunks.
#[derive(Debug, Default)]
pub struct SseParser {
    /// The bytes of the line not yet ended.
    line: Vec<u8>,
    /// The previous chunk ended on `\r`, so a leading `\n` is that line's.
    skip_lf: bool,
    data: String,
    has_data: bool,
    id: Option<String>,
    name: Option<String>,
    comment: Option<String>,
    retry_ms: Option<u64>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes of the line still being read (for a memory bound).
    pub fn pending_bytes(&self) -> usize {
        self.line.len() + self.data.len()
    }

    /// Feed one chunk; every event it completes is pushed to `out`.
    pub fn feed(&mut self, chunk: &[u8], out: &mut Vec<SseEvent>) {
        for &b in chunk {
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\n' => self.end_line(out),
                b'\r' => {
                    self.end_line(out);
                    self.skip_lf = true;
                }
                other => self.line.push(other),
            }
        }
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) {
        let bytes = std::mem::take(&mut self.line);
        let line = String::from_utf8_lossy(&bytes);
        self.line_read(&line, out);
    }

    fn line_read(&mut self, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            if self.has_data {
                if self.data.ends_with('\n') {
                    self.data.pop();
                }
                let data = std::mem::take(&mut self.data);
                out.push(SseEvent {
                    data: (!data.is_empty()).then_some(data),
                    id: self.id.take(),
                    name: self.name.take(),
                    comment: self.comment.take(),
                    retry_ms: self.retry_ms.take(),
                });
            }
            self.data.clear();
            self.has_data = false;
            self.id = None;
            self.name = None;
            self.comment = None;
            self.retry_ms = None;
            return;
        }
        if let Some(rest) = line.strip_prefix(':') {
            self.comment = Some(strip_leading_space(rest).to_string());
            return;
        }
        let (field, value) = match line.find(':') {
            Some(i) => (&line[..i], strip_leading_space(&line[i + 1..])),
            None => (line, ""),
        };
        match field {
            "data" => {
                self.has_data = true;
                self.data.push_str(value);
                self.data.push('\n');
            }
            "id" => {
                if !value.contains('\0') {
                    self.id = Some(value.to_string());
                }
            }
            "event" => self.name = Some(value.to_string()),
            // Only all ASCII digits, and Java's `Long.parseLong`: an
            // overflow is ignored.
            "retry" if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) => {
                if let Ok(ms) = value.parse::<i64>() {
                    self.retry_ms = Some(ms as u64);
                }
            }
            _ => {}
        }
    }
}

fn strip_leading_space(v: &str) -> &str {
    v.strip_prefix(' ').unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_chunks(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        for c in chunks {
            p.feed(c, &mut out);
        }
        out
    }

    fn data(s: &str) -> SseEvent {
        SseEvent {
            data: Some(s.to_string()),
            ..SseEvent::default()
        }
    }

    #[test]
    fn events_are_dispatched_on_an_empty_line_with_every_field() {
        let got = parse_chunks(&[
            b": hello\nid: 7\nevent: tick\nretry: 3000\ndata: {\"a\":1}\n\n",
            b"data:no-space\ndata:  two spaces\n\n",
        ]);
        assert_eq!(
            got,
            vec![
                SseEvent {
                    data: Some("{\"a\":1}".into()),
                    id: Some("7".into()),
                    name: Some("tick".into()),
                    comment: Some("hello".into()),
                    retry_ms: Some(3000),
                },
                data("no-space\n two spaces"),
            ]
        );
    }

    #[test]
    fn every_line_ending_works_and_may_straddle_chunks() {
        assert_eq!(
            parse_chunks(&[b"data: a\r\n\r\ndata: b\r\rdata: c\n", b"\n"]),
            vec![data("a"), data("b"), data("c")]
        );
        // `\r` at the end of one chunk, `\n` at the start of the next: one line end.
        assert_eq!(
            parse_chunks(&[b"data: x\r", b"\ndata: y\r", b"\n\r", b"\n"]),
            vec![data("x\ny")]
        );
        // A multi-byte character split across chunks.
        let bytes = "data: é\n\n".as_bytes();
        assert_eq!(parse_chunks(&[&bytes[..7], &bytes[7..]]), vec![data("é")]);
    }

    #[test]
    fn no_data_no_event_and_fields_reset() {
        assert_eq!(
            parse_chunks(&[b"id: 1\nevent: e\n\ndata: z\n\n"]),
            vec![data("z")],
            "an event without data is dropped, and its id does not leak"
        );
        // An empty data line is an event whose data is null.
        assert_eq!(parse_chunks(&[b"data:\n\n"]), vec![SseEvent::default()]);
    }

    #[test]
    fn the_end_of_the_stream_does_not_dispatch() {
        assert_eq!(
            parse_chunks(&[b"data: a\n\ndata: pending\n"]),
            vec![data("a")]
        );
    }

    #[test]
    fn odd_fields_follow_kestra() {
        let got = parse_chunks(&[
            b"id: a\0b\nretry: 12x\nretry: 99999999999999999999\nunknown: v\ndata\n\n",
        ]);
        // `data` with no colon: an empty value, so the data is "" -> null.
        assert_eq!(got, vec![SseEvent::default()]);
        let got = parse_chunks(&[b"retry: 5\nretry: 12x\ndata: d\n\n"]);
        assert_eq!(
            got[0].retry_ms,
            Some(5),
            "a bad retry keeps the last good one"
        );
    }
}
