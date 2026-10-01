//! The Ion READER of the execution file API's preview (W9a spec §8).
//!
//! `ion-rs` 1.0.1 over bytes, **one top-level value at a time, text or
//! binary**: Kestra's STORE and item-task files are binary Ion 1.0 with a
//! version marker and a local symbol table before every row and no lines
//! (`w9a-storage.json` finding `store_binary_ion`); `Write`'s and `ionfmt`'s
//! are text. Nothing here splits on `'\n'`, and nothing reads past the value
//! that fills the page (plus one, to know whether more follow).
//!
//! # The JSON a value becomes (Kestra's preview rows, measured)
//!
//! | Ion | JSON | measured |
//! |---|---|---|
//! | struct | object; a field whose value is any `null` is **dropped** | `n:null` absent from every row |
//! | list, sexp | array; a `null` element **stays** `null` | `l:[null]` |
//! | timestamp | its Ion text as a string, a day-precision date without the `T` | `"2025-12-03"`, `"2025-12-03T12:00:00.123+07:00"` |
//! | decimal | a JSON number, as a double | `12.50` -> `12.5`, `1d2` -> `100.0` |
//! | float | a JSON number; NaN and the infinities as strings | `1.5e0` -> `1.5`, `2e0` -> `2.0` |
//! | int | a JSON number, exact at any size | `9007199254740993` |
//! | string, symbol | a string | |
//! | blob, clob | base64 of the bytes, as a `byte[]` is | (chosen; `runners::ionfmt`'s table) |
//! | annotations | dropped: `LocalDateTime::"…"` is the string | (chosen) |
//! | top-level `null` | `null` | |

use std::io::Read;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Why reading stopped.
#[derive(Debug)]
pub(crate) enum Stop {
    /// `max` values were read and another follows.
    Full,
    /// The input ended after the last value read.
    End,
    /// The input is not valid Ion, or the reader failed, after the values read.
    Failed(String),
    /// A container nests deeper than [`MAX_DEPTH`]: refused before the Ion
    /// reader saw the byte that opened it.
    TooDeep(String),
    /// The reader's thread could not start, or panicked.
    Crashed(String),
}

/// The deepest nesting of containers (list, sexp, struct, and in binary an
/// annotation wrapper) a file may have to be read here at all.
pub const MAX_DEPTH: usize = 100;

/// Up to `max` top-level values of `bytes` as JSON, and whether more follow.
///
/// `Err` names why the input is not Ion; the values before the fault are lost
/// (see [`read_values`] for a caller that keeps them).
pub fn values_to_json(
    bytes: impl Read + Send + 'static,
    max: usize,
) -> Result<(Vec<Value>, bool /*truncated*/), String> {
    match read_values(bytes, max) {
        (rows, Stop::Full) => Ok((rows, true)),
        (rows, Stop::End) => Ok((rows, false)),
        (_, Stop::Failed(e) | Stop::TooDeep(e) | Stop::Crashed(e)) => Err(e),
    }
}

/// [`values_to_json`], keeping the values read before a fault -- **the one
/// way into the Ion reader** (the preview and Task 11's `Loop` over a file
/// both come through it).
///
/// - **Depth-safe by construction.** The bytes pass a depth scan (text and
///   binary Ion, [`DepthGuard`]) before the reader sees them; the byte that
///   would open a container past [`MAX_DEPTH`] is never handed over, so the
///   reader -- which recurses once a level -- and [`to_json`] never go deeper.
///   It runs on a dedicated thread whose stack ([`READER_STACK_BYTES`]) is
///   many times what `MAX_DEPTH` levels take (the review measured about 700
///   levels in tokio's 2 MiB blocking stack).
/// - **Linear in the size of a value.** The reader re-parses the value it is
///   in after every fill of its buffer, and grows the buffer only when a fill
///   filled it; [`FillWhole`] fills every request, so a value of `n` bytes is
///   parsed about `log2(n / 4 KiB)` times instead of once per store chunk.
/// - Reads one value past `max` only to learn whether more follow; any
///   outcome of that read (another value, an error, a refusal) is `Full`.
/// - A reader that fails mid-value (the preview's byte budget does, on
///   purpose) never yields that value cut short: the Ion reader asks for more
///   bytes before it ends a token, and gets the error.
pub(crate) fn read_values<R: Read + Send + 'static>(bytes: R, max: usize) -> (Vec<Value>, Stop) {
    let spawned = std::thread::Builder::new()
        .name("egret-ion-reader".into())
        .stack_size(READER_STACK_BYTES)
        .spawn(move || read_guarded(bytes, max));
    match spawned {
        Err(e) => (
            Vec::new(),
            Stop::Crashed(format!("the Ion reader's thread did not start: {e}")),
        ),
        Ok(handle) => handle.join().unwrap_or_else(|panic| {
            let why = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no message".into());
            (
                Vec::new(),
                Stop::Crashed(format!("the Ion reader panicked: {why}")),
            )
        }),
    }
}

/// The stack of the reader's thread: `MAX_DEPTH` levels need well under 1 MiB
/// in a debug build.
const READER_STACK_BYTES: usize = 16 << 20;

fn read_guarded<R: Read>(bytes: R, max: usize) -> (Vec<Value>, Stop) {
    let trip: Arc<Mutex<Option<Trip>>> = Arc::default();
    let guarded = FillWhole {
        inner: DepthGuard {
            inner: bytes,
            scan: Scan::Undecided,
            trip: None,
            shared: trip.clone(),
        },
        pending: None,
    };
    // Capacity 0: every read goes straight through to `FillWhole`, whole.
    let input = std::io::BufReader::with_capacity(0, guarded);
    let tripped = || trip.lock().ok().and_then(|mut g| g.take());
    let mut values = match ion_rs::Element::iter(input) {
        Ok(it) => it,
        Err(e) => {
            return match tripped() {
                Some(t) => (Vec::new(), t.into_stop()),
                None => (Vec::new(), Stop::Failed(e.to_string())),
            }
        }
    };
    let mut rows = Vec::new();
    loop {
        match values.next() {
            None => return (rows, Stop::End),
            Some(_) if rows.len() == max => return (rows, Stop::Full),
            Some(Err(e)) => {
                return match tripped() {
                    Some(t) => (rows, t.into_stop()),
                    None => (rows, Stop::Failed(e.to_string())),
                }
            }
            Some(Ok(element)) => rows.push(to_json(&element)),
        }
    }
}

/// Why the [`DepthGuard`] stopped handing bytes over.
#[derive(Clone, Debug)]
enum Trip {
    TooDeep(String),
    /// Binary Ion the scan cannot follow (a reserved type code, an overlong
    /// length): refused rather than passed on unscanned.
    Malformed(String),
}

impl Trip {
    fn message(&self) -> &str {
        match self {
            Trip::TooDeep(m) | Trip::Malformed(m) => m,
        }
    }
    fn into_stop(self) -> Stop {
        match self {
            Trip::TooDeep(m) => Stop::TooDeep(m),
            Trip::Malformed(m) => Stop::Failed(m),
        }
    }
}

fn too_deep() -> Trip {
    Trip::TooDeep(format!(
        "the file nests containers deeper than {MAX_DEPTH} levels, which egret does not read"
    ))
}

/// Fills every request whole (or to the end of the input). An error after
/// some bytes is returned on the NEXT call, after those bytes.
struct FillWhole<R> {
    inner: R,
    pending: Option<std::io::Error>,
}

impl<R: Read> Read for FillWhole<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(e) = self.pending.take() {
            return Err(e);
        }
        let mut n = 0;
        while n < buf.len() {
            match self.inner.read(&mut buf[n..]) {
                Ok(0) => break,
                Ok(k) => n += k,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) if n == 0 => return Err(e),
                Err(e) => {
                    self.pending = Some(e);
                    break;
                }
            }
        }
        Ok(n)
    }
}

/// Scans every byte before handing it on; the byte that trips the scan and
/// everything after it are never handed on (the call that met it returns the
/// bytes before it, and every later call the error).
struct DepthGuard<R> {
    inner: R,
    scan: Scan,
    trip: Option<Trip>,
    shared: Arc<Mutex<Option<Trip>>>,
}

impl<R: Read> Read for DepthGuard<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(t) = &self.trip {
            return Err(std::io::Error::other(t.message().to_string()));
        }
        let n = self.inner.read(buf)?;
        for (i, b) in buf[..n].iter().enumerate() {
            if let Err(t) = self.scan.feed(*b) {
                if let Ok(mut g) = self.shared.lock() {
                    *g = Some(t.clone());
                }
                let message = t.message().to_string();
                self.trip = Some(t);
                return if i == 0 {
                    Err(std::io::Error::other(message))
                } else {
                    Ok(i)
                };
            }
        }
        Ok(n)
    }
}

/// The depth scan, text or binary as the first byte says (binary Ion starts
/// with its version marker, `E0`; text Ion never does).
enum Scan {
    Undecided,
    Text(TextScan),
    Binary(BinaryScan),
}

impl Scan {
    fn feed(&mut self, b: u8) -> Result<(), Trip> {
        if let Scan::Undecided = self {
            *self = if b == 0xE0 {
                Scan::Binary(BinaryScan::default())
            } else {
                Scan::Text(TextScan::default())
            };
        }
        match self {
            Scan::Text(t) => t.feed(b),
            Scan::Binary(s) => s.feed(b),
            Scan::Undecided => unreachable!("decided above"),
        }
    }
}

/// Text Ion: `[`, `(` and `{` open a level and `]`, `)` and `}` close one.
///
/// **Safety is structural: the scan skips only what it can prove is data** --
/// a quoted string, a quoted symbol, a long string, a lob (fix round 3,
/// controller ruling). Everything else counts its opening brackets, the
/// inside of a comment included:
///
/// - Inside `//…` or `/*…*/` an opening bracket counts and a closing one does
///   not. However `ion-rs` reads the comment, the scan is never behind it:
///   `ion-rs` 1.0.1 reads a `/*` with no `*/` after it as an s-expression
///   OPERATOR and parses what follows (`(/* [b])` is `('/*' [b])`), and the
///   scan, still in its comment, counts every level of it. The price: a
///   shallow file with many brackets in comments can be refused (Kestra writes
///   no comments).
/// - A comment ends where `ion-rs` ends one (probed): a line comment at `\n`
///   or `\r`, a block comment at the first `*/` after its `/*`. Within a
///   comment no quote starts a string.
/// - `//` and `/*` start a comment unless the `/` directly follows an
///   operator character (`!#%&*+-./;<=>?@^`|~`): there `ion-rs` reads it as
///   part of an s-expression operator (`(+// [])` is `('+//' [])`), and code
///   follows -- possibly a long string whose closing brackets must not count.
///   Outside an s-expression an operator character before `//` is a syntax
///   error, where the reader stops.
/// - `{{` opens a lob, which is not a level: a blob's base64 (`/` and `+`
///   included) is data, a clob's `"…"` and `'''…'''` keep their string rules,
///   and `}}` closes it. Its first `{` is counted until the second is seen,
///   so the scan is never behind the reader. Inside a comment `{{` is two
///   opening brackets.
///
/// What the reader would refuse (a malformed lob, a stray `}`) may be
/// scanned differently: the reader stops at the fault and parses nothing
/// after it.
#[derive(Default)]
struct TextScan {
    depth: usize,
    state: Text,
    /// The last byte scanned in `Normal` was an operator character.
    after_operator: bool,
}

#[derive(Clone, Copy, Default)]
enum Text {
    #[default]
    Normal,
    /// A `/` not after an operator character: a comment may start.
    Slash,
    Str,
    StrEscape,
    /// One `'`: a quoted symbol, or the start of `'''`.
    Quote1,
    /// `''`: an empty symbol, or the start of `'''`.
    Quote2,
    Symbol,
    SymbolEscape,
    /// Inside `'''…'''`, with the number of `'` just seen.
    Long(u8),
    LongEscape,
    LineComment,
    /// Inside `/*…*/`; `true` right after a `*`.
    BlockComment(bool),
    /// A `{` (counted as a level): a struct, or the start of `{{`.
    Brace,
    /// Inside `{{…}}`, between its strings.
    Lob,
    LobStr,
    LobStrEscape,
    /// `'` or `''` inside a lob: the start of a clob's `'''`.
    LobQuote(u8),
    LobLong(u8),
    LobLongEscape,
    /// One `}` inside a lob.
    LobClose,
}

fn is_operator(b: u8) -> bool {
    matches!(
        b,
        b'!' | b'#'
            | b'%'
            | b'&'
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'/'
            | b';'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'^'
            | b'`'
            | b'|'
            | b'~'
    )
}

impl TextScan {
    fn open(&mut self) -> Result<(), Trip> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(too_deep());
        }
        Ok(())
    }

    fn normal(&mut self, b: u8) -> Result<Text, Trip> {
        let after_operator = self.after_operator;
        self.after_operator = false;
        Ok(match b {
            b'"' => Text::Str,
            b'\'' => Text::Quote1,
            b'/' if !after_operator => Text::Slash,
            b'{' => {
                self.open()?;
                Text::Brace
            }
            b'[' | b'(' => {
                self.open()?;
                Text::Normal
            }
            b']' | b')' | b'}' => {
                self.depth = self.depth.saturating_sub(1);
                Text::Normal
            }
            _ => {
                self.after_operator = is_operator(b);
                Text::Normal
            }
        })
    }

    fn feed(&mut self, b: u8) -> Result<(), Trip> {
        self.state = match self.state {
            Text::Normal => self.normal(b)?,
            Text::Slash => match b {
                b'/' => Text::LineComment,
                b'*' => Text::BlockComment(false),
                // The `/` was an operator (`(a / b)`).
                _ => {
                    self.after_operator = true;
                    self.normal(b)?
                }
            },
            Text::Str => match b {
                b'\\' => Text::StrEscape,
                b'"' => Text::Normal,
                _ => Text::Str,
            },
            Text::StrEscape => Text::Str,
            Text::Quote1 => match b {
                b'\'' => Text::Quote2,
                b'\\' => Text::SymbolEscape,
                _ => Text::Symbol,
            },
            Text::Quote2 => match b {
                b'\'' => Text::Long(0),
                _ => self.normal(b)?,
            },
            Text::Symbol => match b {
                b'\\' => Text::SymbolEscape,
                b'\'' => Text::Normal,
                _ => Text::Symbol,
            },
            Text::SymbolEscape => Text::Symbol,
            Text::Long(quotes) => match b {
                b'\\' => Text::LongEscape,
                b'\'' if quotes == 2 => Text::Normal,
                b'\'' => Text::Long(quotes + 1),
                _ => Text::Long(0),
            },
            Text::LongEscape => Text::Long(0),
            // In a comment an opening bracket still counts (see the type's doc).
            Text::LineComment => match b {
                b'\n' | b'\r' => Text::Normal,
                b'[' | b'(' | b'{' => {
                    self.open()?;
                    Text::LineComment
                }
                _ => Text::LineComment,
            },
            Text::BlockComment(star) => match b {
                b'/' if star => Text::Normal,
                b'[' | b'(' | b'{' => {
                    self.open()?;
                    Text::BlockComment(false)
                }
                _ => Text::BlockComment(b == b'*'),
            },
            Text::Brace => match b {
                // `{{`: a lob, not a level.
                b'{' => {
                    self.depth -= 1;
                    Text::Lob
                }
                _ => self.normal(b)?,
            },
            Text::Lob => match b {
                b'"' => Text::LobStr,
                b'\'' => Text::LobQuote(1),
                b'}' => Text::LobClose,
                _ => Text::Lob,
            },
            Text::LobStr => match b {
                b'\\' => Text::LobStrEscape,
                b'"' => Text::Lob,
                _ => Text::LobStr,
            },
            Text::LobStrEscape => Text::LobStr,
            Text::LobQuote(n) => match b {
                b'\'' if n == 2 => Text::LobLong(0),
                b'\'' => Text::LobQuote(n + 1),
                _ => Text::Lob,
            },
            Text::LobLong(quotes) => match b {
                b'\\' => Text::LobLongEscape,
                b'\'' if quotes == 2 => Text::Lob,
                b'\'' => Text::LobLong(quotes + 1),
                _ => Text::LobLong(0),
            },
            Text::LobLongEscape => Text::LobLong(0),
            Text::LobClose => match b {
                b'}' => Text::Normal,
                _ => Text::Lob,
            },
        };
        Ok(())
    }
}

/// Binary Ion 1.0: follows every type descriptor and length, and keeps one
/// frame per open list, sexp, struct and annotation wrapper (the wrapper is
/// counted as a level: too deep, never too shallow).
#[derive(Default)]
struct BinaryScan {
    /// Index of the byte being fed.
    pos: u64,
    state: Binary,
    stack: Vec<Frame>,
}

struct Frame {
    /// Index of the first byte after the container.
    end: u64,
    kind: Kind,
}

#[derive(Clone, Copy)]
enum Kind {
    Sequence,
    Struct,
    Annotation { header_read: bool },
}

#[derive(Clone, Copy, Default)]
enum Binary {
    /// The next byte starts an item of the innermost container (or of the
    /// top level).
    #[default]
    Item,
    FieldId(u64),
    TypeDescriptor,
    Length(u64, Then),
    AnnotationsLength(u64),
    Skip(u64),
}

#[derive(Clone, Copy)]
enum Then {
    Skip,
    Open(Kind),
}

/// One byte of a VarUInt: `Some(value)` on its last byte.
fn var_uint(acc: u64, b: u8) -> Result<(u64, bool), Trip> {
    if acc >> 56 != 0 {
        return Err(Trip::Malformed(
            "a binary Ion length is longer than egret reads".into(),
        ));
    }
    Ok(((acc << 7) | u64::from(b & 0x7F), b & 0x80 != 0))
}

impl BinaryScan {
    /// A value of `len` bytes from here must end inside its container: a
    /// length that runs past it is refused before the reader sees the byte
    /// that completes it (fix round 2 -- `ion-rs` subtracts such lengths
    /// unchecked; in a debug build that panics).
    fn fits(&self, len: u64) -> Result<(), Trip> {
        match self.stack.last() {
            Some(f) if self.pos.saturating_add(len) > f.end => Err(Trip::Malformed(
                "a binary Ion value runs past the container that holds it".into(),
            )),
            _ => Ok(()),
        }
    }

    fn open(&mut self, kind: Kind, len: u64) -> Result<Binary, Trip> {
        self.fits(len)?;
        // `self.pos` is already past the byte that ended the header.
        self.stack.push(Frame {
            end: self.pos + len,
            kind,
        });
        if self.stack.len() > MAX_DEPTH {
            return Err(too_deep());
        }
        Ok(Binary::Item)
    }

    fn skip(&self, len: u64) -> Result<Binary, Trip> {
        self.fits(len)?;
        Ok(if len == 0 {
            Binary::Item
        } else {
            Binary::Skip(len)
        })
    }

    fn after_length(&mut self, len: u64, then: Then) -> Result<Binary, Trip> {
        match then {
            Then::Skip => self.skip(len),
            Then::Open(kind) => self.open(kind, len),
        }
    }

    fn type_descriptor(&mut self, b: u8) -> Result<Binary, Trip> {
        if self.stack.is_empty() && b == 0xE0 {
            // A version marker: E0 01 00 EA.
            return Ok(Binary::Skip(3));
        }
        let (t, l) = (b >> 4, u64::from(b & 0x0F));
        Ok(match t {
            // null/pad, int, float, decimal, timestamp, symbol, string, clob, blob.
            0 | 2..=10 => match l {
                15 => Binary::Item,
                14 => Binary::Length(0, Then::Skip),
                _ => self.skip(l)?,
            },
            // bool: its value is in the descriptor.
            1 => Binary::Item,
            11..=13 => {
                let kind = if t == 13 {
                    Kind::Struct
                } else {
                    Kind::Sequence
                };
                match l {
                    15 => Binary::Item,
                    14 => Binary::Length(0, Then::Open(kind)),
                    1 if t == 13 => Binary::Length(0, Then::Open(kind)),
                    _ => self.open(kind, l)?,
                }
            }
            14 if l == 14 => Binary::Length(0, Then::Open(Kind::Annotation { header_read: false })),
            14 if (3..14).contains(&l) => self.open(Kind::Annotation { header_read: false }, l)?,
            _ => {
                return Err(Trip::Malformed(format!(
                    "the binary Ion type descriptor {b:#04x} is not one egret reads"
                )))
            }
        })
    }

    fn feed(&mut self, b: u8) -> Result<(), Trip> {
        let pos = self.pos;
        self.pos += 1;
        if let Binary::Item = self.state {
            while self.stack.last().is_some_and(|f| f.end <= pos) {
                self.stack.pop();
            }
            self.state = match self.stack.last_mut() {
                Some(Frame {
                    kind: Kind::Struct, ..
                }) => Binary::FieldId(0),
                Some(Frame {
                    kind: Kind::Annotation { header_read },
                    ..
                }) if !*header_read => {
                    *header_read = true;
                    Binary::AnnotationsLength(0)
                }
                _ => Binary::TypeDescriptor,
            };
        }
        self.state = match self.state {
            Binary::Item => unreachable!("resolved above"),
            Binary::FieldId(acc) => match var_uint(acc, b)? {
                (_, true) => Binary::TypeDescriptor,
                (acc, false) => Binary::FieldId(acc),
            },
            Binary::TypeDescriptor => self.type_descriptor(b)?,
            Binary::Length(acc, then) => match var_uint(acc, b)? {
                (len, true) => self.after_length(len, then)?,
                (acc, false) => Binary::Length(acc, then),
            },
            Binary::AnnotationsLength(acc) => match var_uint(acc, b)? {
                // At least one annotation, and room for the value after them.
                (len, true) => {
                    if len == 0 || self.fits(len.saturating_add(1)).is_err() {
                        return Err(Trip::Malformed(
                            "a binary Ion annotation wrapper's annotations do not fit in it".into(),
                        ));
                    }
                    self.skip(len)?
                }
                (acc, false) => Binary::AnnotationsLength(acc),
            },
            Binary::Skip(1) => Binary::Item,
            Binary::Skip(n) => Binary::Skip(n - 1),
        };
        Ok(())
    }
}

fn to_json(element: &ion_rs::Element) -> Value {
    use base64::Engine as _;
    use ion_rs::Value as V;
    let b64 = |b: &[u8]| Value::String(base64::engine::general_purpose::STANDARD.encode(b));
    match element.value() {
        V::Null(_) => Value::Null,
        V::Bool(b) => Value::Bool(*b),
        V::Int(i) => match i.as_i64() {
            Some(n) => Value::from(n),
            // Exact at any size (this crate's serde_json is arbitrary_precision).
            None => serde_json::from_str::<serde_json::Number>(&i.to_string())
                .map(Value::Number)
                .unwrap_or_else(|_| Value::String(i.to_string())),
        },
        V::Float(f) => float(*f),
        V::Decimal(d) => {
            // As a double, as Kestra's preview shows it (12.50 -> 12.5, 1d2 -> 100.0).
            // `Decimal`'s Display is Ion text (`-0.001`, `1d2`); `d` is the exponent.
            let text = d.to_string();
            match text.replace(['d', 'D'], "e").parse::<f64>() {
                Ok(f) => float(f),
                Err(_) => Value::String(text),
            }
        }
        V::Timestamp(t) => {
            let text = t.to_string();
            // ion-java writes a day-precision timestamp without its `T`.
            if t.precision() == ion_rs::TimestampPrecision::Day {
                Value::String(text.trim_end_matches('T').to_string())
            } else if let Some(utc) = text.strip_suffix("+00:00") {
                // ... and UTC as `Z` (`-00:00`, an unknown offset, stays).
                Value::String(format!("{utc}Z"))
            } else {
                Value::String(text)
            }
        }
        V::Symbol(s) => s
            .text()
            .map(|t| Value::String(t.to_string()))
            .unwrap_or(Value::Null),
        V::String(s) => Value::String(s.text().to_string()),
        V::Clob(b) | V::Blob(b) => b64(b.as_ref()),
        V::List(seq) | V::SExp(seq) => Value::Array(seq.elements().map(to_json).collect()),
        V::Struct(st) => {
            let mut map = serde_json::Map::new();
            for (name, value) in st.fields() {
                if value.is_null() {
                    continue;
                }
                map.insert(name.text().unwrap_or("").to_string(), to_json(value));
            }
            Value::Object(map)
        }
    }
}

fn float(f: f64) -> Value {
    if f.is_nan() {
        Value::String("NaN".into())
    } else if f.is_infinite() {
        Value::String(if f > 0.0 { "Infinity" } else { "-Infinity" }.into())
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8 MiB in one value. Measured in fix round 1 (`ion-rs` at opt-level 3 in
    /// dev, the workspace's `[profile.dev.package.ion-rs]`): about 7 s; the
    /// quadratic reader had not finished after 31 minutes.
    const TIME_BOUND_SECS: u64 = 20;
    use base64::Engine as _;
    use serde_json::json;

    fn oracle() -> Value {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/oracle-exec/w9a-storage.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn b64(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
    }

    /// `file_api.ion_preview_source_text` (the text Ion Kestra previewed)
    /// gives `file_api.preview_maxRows_99999`'s rows, and `maxRows=2` its
    /// `preview_maxRows_2` rows, truncated.
    #[test]
    fn kestras_ion_preview_rows_are_reproduced_value_for_value() {
        let o = oracle();
        let fa = &o["egret_expected"]["file_api"];
        let text = fa["ion_preview_source_text"].as_str().unwrap();
        let all = fa["preview_maxRows_99999"]["body"]["content"].clone();
        let (rows, truncated) = values_to_json(cursor(text), 5000).unwrap();
        assert_eq!(Value::Array(rows), all);
        assert!(!truncated);

        let two = fa["files"]["ion"]["preview_maxRows_2"]["body"]["content"].clone();
        let (rows, truncated) = values_to_json(cursor(text), 2).unwrap();
        assert_eq!(Value::Array(rows), two);
        assert!(truncated);

        // Exactly as many values as asked for is not truncated.
        let (rows, truncated) = values_to_json(cursor(text), 3).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(!truncated);
    }

    /// The decoded values of what each `ion_rows` row should preview as.
    fn expected_row(sql_type: &str) -> Value {
        match sql_type {
            "int" => json!({"v": 42}),
            "bigint" => json!({"v": 9007199254740993_u64}),
            "decimal" => json!({"v": 12.34}),
            "float" => json!({"v": 1.5}),
            "bool" => json!({"v": true}),
            "text_with_quote" => json!({"v": "say \"hi\""}),
            "timestamp" => json!({"v": "2026-01-02T03:04:05.123456"}),
            "timestamptz" => json!({"v": "2026-01-01T20:04:05Z[Etc/UTC]"}),
            "date" => json!({"v": "2026-01-02"}),
            "null" => json!({}),
            "bytea" => json!({"v": "3q2+7w=="}),
            other => panic!("no expectation for {other}"),
        }
    }

    /// `ion_rows`: Kestra's BINARY STORE rows decode to the same values as
    /// their `ion_text`, and to the expected preview value -- decoded values
    /// compared, never bytes.
    #[test]
    fn binary_store_rows_decode_as_their_text_does() {
        let o = oracle();
        let rows = o["egret_expected"]["ion_rows"].as_array().unwrap();
        assert_eq!(rows.len(), 11);
        for r in rows {
            let ty = r["sql_type"].as_str().unwrap();
            let bytes = b64(r["bytes_b64"].as_str().unwrap());
            assert_eq!(&bytes[..4], &[0xe0, 0x01, 0x00, 0xea], "{ty}: binary Ion");
            let (bin, t1) = values_to_json(cursor(bytes.clone()), 100).unwrap();
            let (txt, t2) = values_to_json(cursor(r["ion_text"].as_str().unwrap()), 100).unwrap();
            assert_eq!(bin, txt, "{ty}: binary and text decode alike");
            assert_eq!(bin, vec![expected_row(ty)], "{ty}");
            assert!(!t1 && !t2);
        }
    }

    /// A STORE file is every row's bytes one after another, each with its own
    /// version marker and symbol table: read value by value, all 11 come back,
    /// and `max` stops after the asked number.
    #[test]
    fn a_binary_file_of_many_version_markers_reads_value_by_value() {
        let o = oracle();
        let rows = o["egret_expected"]["ion_rows"].as_array().unwrap();
        let mut file = Vec::new();
        let mut want = Vec::new();
        for r in rows {
            file.extend(b64(r["bytes_b64"].as_str().unwrap()));
            want.push(expected_row(r["sql_type"].as_str().unwrap()));
        }
        let (all, truncated) = values_to_json(cursor(file.clone()), 100).unwrap();
        assert_eq!(all, want);
        assert!(!truncated);
        let (some, truncated) = values_to_json(cursor(file.clone()), 4).unwrap();
        assert_eq!(some, want[..4].to_vec());
        assert!(truncated);
    }

    #[test]
    fn not_ion_is_an_error_and_the_values_before_it_are_kept_by_read_values() {
        assert!(values_to_json(&b"{a:1} {b:"[..], 10).is_err());
        let (rows, stop) = read_values(&b"{a:1} {b:"[..], 10);
        assert_eq!(rows, vec![json!({"a": 1})]);
        assert!(matches!(stop, Stop::Failed(_)), "{stop:?}");
    }

    /// A reader that fails after `n` bytes, as the preview's byte budget does.
    struct FailsAfter(&'static [u8], usize);
    impl Read for FailsAfter {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1 == 0 {
                return Err(std::io::Error::other("budget"));
            }
            let n = buf.len().min(self.1).min(self.0.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            self.1 -= n;
            Ok(n)
        }
    }

    /// A top-level text scalar cut by the budget is never a (wrong) value:
    /// `123456` cut after `12` must not preview as `12`.
    #[test]
    fn a_value_cut_by_a_failing_reader_is_not_read_as_a_shorter_one() {
        let (rows, stop) = read_values(FailsAfter(b"1 2 123456 7", 6), 10);
        assert_eq!(rows, vec![json!(1), json!(2)]);
        assert!(matches!(stop, Stop::Failed(_)), "{stop:?}");
    }

    /// `depth` lists, each inside the next, in BINARY Ion (after a version marker).
    fn binary_nesting(depth: usize) -> Vec<u8> {
        let header = |len: usize| -> Vec<u8> {
            if len < 14 {
                return vec![0xB0 | len as u8];
            }
            // VarUInt: 7 bits a byte, big-endian, the last byte's high bit set.
            let mut groups = Vec::new();
            let mut n = len;
            while n > 0 || groups.is_empty() {
                groups.push((n & 0x7F) as u8);
                n >>= 7;
            }
            groups.reverse();
            *groups.last_mut().unwrap() |= 0x80;
            let mut out = vec![0xBE];
            out.extend(groups);
            out
        };
        // Innermost first: an empty list, then each level wraps the one inside.
        let mut headers = vec![header(0)];
        let mut size = 1usize;
        for _ in 1..depth {
            let h = header(size);
            size += h.len();
            headers.push(h);
        }
        let mut out = vec![0xE0, 0x01, 0x00, 0xEA];
        for h in headers.iter().rev() {
            out.extend(h);
        }
        out
    }

    fn nested_json(depth: usize) -> Value {
        let mut v = json!([]);
        for _ in 1..depth {
            v = json!([v]);
        }
        v
    }

    fn cursor(bytes: impl Into<Vec<u8>>) -> std::io::Cursor<Vec<u8>> {
        std::io::Cursor::new(bytes.into())
    }

    /// Fix round 1, C1: `[`×100_000 aborted the process (the reader recurses
    /// once a level, on a 2 MiB stack). Refused in-process, before the reader
    /// saw it; the value before it stands.
    #[test]
    fn text_nesting_past_the_cap_is_refused_not_a_crash() {
        let deep = format!("{{a:1}} {}{}", "[".repeat(100_000), "]".repeat(100_000));
        let (rows, stop) = read_values(cursor(deep), 10);
        assert_eq!(rows, vec![json!({"a": 1})]);
        assert!(matches!(stop, Stop::TooDeep(_)), "{stop:?}");
        assert!(values_to_json(cursor("[".repeat(MAX_DEPTH + 1)), 10).is_err());
    }

    #[test]
    fn binary_nesting_past_the_cap_is_refused_not_a_crash() {
        let (rows, stop) = read_values(cursor(binary_nesting(100_000)), 10);
        assert!(rows.is_empty());
        assert!(matches!(stop, Stop::TooDeep(_)), "{stop:?}");
    }

    /// Control: exactly `MAX_DEPTH` levels read, text and binary alike; one
    /// more is refused.
    #[test]
    fn nesting_at_the_cap_reads() {
        let text = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        let (rows, stop) = read_values(cursor(text), 10);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows, vec![nested_json(MAX_DEPTH)]);
        let (rows, stop) = read_values(cursor(binary_nesting(MAX_DEPTH)), 10);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows, vec![nested_json(MAX_DEPTH)]);
        let (_, stop) = read_values(cursor(binary_nesting(MAX_DEPTH + 1)), 10);
        assert!(matches!(stop, Stop::TooDeep(_)), "{stop:?}");
    }

    /// Brackets inside strings, quoted symbols and long strings are not
    /// nesting; closing brackets there or in a comment never hide real nesting.
    #[test]
    fn the_depth_count_reads_strings_symbols_and_comments() {
        let many = "[".repeat(300);
        let text = format!(r#""{many}\"" '{many}\'' '''{many}''' ["]]]"]"#);
        let (rows, stop) = read_values(cursor(text), 10);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows.len(), 4);
        // 50 levels open, then closing brackets the reader does not take as
        // closing, then 51 more levels: 101 in all, whatever those claim.
        let open = "[".repeat(50);
        let deep = "[".repeat(MAX_DEPTH + 1 - 50);
        let closes = "]".repeat(300);
        for hiding in [
            format!("{open}\"{closes}\" {deep}"),
            format!("{open}'{closes}' {deep}"),
            format!("{open}'''{closes}''' {deep}"),
            format!("{open}/* {closes} */ {deep}"),
            format!("{open}// {closes}\n{deep}"),
            format!("{open}(a // {closes}\n{deep}"),
        ] {
            let (_, stop) = read_values(cursor(hiding.clone()), 10);
            assert!(matches!(stop, Stop::TooDeep(_)), "{hiding:.40}: {stop:?}");
        }
    }

    /// Fix round 2: `/` inside a blob is base64, not a comment. The round-1
    /// scan took `ab//cA==` for a comment, stopped counting closing brackets
    /// to the end of the line, and refused 60 valid rows as too deep after 24.
    #[test]
    fn blob_and_clob_rows_with_slashes_read_whole() {
        let row = "{a:{{ab//cA==}}, b:[1], c:{{\"//]]]\"}}, d:{{'''x//''' ''']]'''}}}\n";
        let (rows, stop) = read_values(cursor(row.repeat(600)), 1000);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows.len(), 600);
        assert_eq!(
            rows[599],
            json!({"a": "ab//cA==", "b": [1], "c": "Ly9dXV0=", "d": "eC8vXV0="})
        );
    }

    /// Fix round 3: comments are not skipped. An opening bracket inside one
    /// counts, a closing one does not, so rows with no brackets in their
    /// comments read whole, and brackets in comments are an over-count -- a
    /// shallow file can be refused, never a deep one let through.
    #[test]
    fn comments_count_their_opening_brackets() {
        for (row, want) in [
            ("(a // b c\n)\n", json!(["a"])),
            ("/* a note */ {a:1}\n", json!({"a": 1})),
            ("// a note\n{a:1}\n", json!({"a": 1})),
            ("[1, /* ]]] */ 2]\n", json!([1, 2])),
        ] {
            let (rows, stop) = read_values(cursor(row.repeat(600)), 1000);
            assert!(matches!(stop, Stop::End), "{row:?}: {stop:?}");
            assert_eq!(rows.len(), 600, "{row:?}");
            assert_eq!(rows[599], want, "{row:?}");
        }
        // The reviewer's FP2: each row leaves one level open in the count.
        let fp2 = "(a // b [c]\n)\n";
        let (rows, stop) = read_values(cursor(fp2.repeat(60)), 1000);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows.len(), 60);
        let (_, stop) = read_values(cursor(fp2.repeat(600)), 1000);
        assert!(matches!(stop, Stop::TooDeep(_)), "{stop:?}");
    }

    /// Fix round 3, C1 brought back by round 2: `ion-rs` reads a `/*` with no
    /// `*/` after it as an s-expression operator and parses what follows. The
    /// round-2 scan skipped it as a comment; 5000 levels after it aborted a
    /// debug build. Every opening bracket after it now counts.
    #[test]
    fn an_unterminated_block_comment_hides_no_nesting() {
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        for (name, text, rows_before) in [
            ("101 levels", format!("(/* {})", deep(MAX_DEPTH + 1)), 0),
            ("10 KB", format!("(/* {})", deep(5000)), 0),
            ("100k levels", format!("(/* {})", deep(100_000)), 0),
            ("after a row", format!("{{a:1}}\n(/* {})", deep(5000)), 1),
            ("then a tail", format!("(a /* {}) /* tail", deep(5000)), 0),
            (
                "line comment, no newline",
                format!("(a // {})", deep(5000)),
                0,
            ),
        ] {
            let (rows, stop) = read_values(cursor(text), 10);
            assert_eq!(rows.len(), rows_before, "{name}");
            assert!(matches!(stop, Stop::TooDeep(_)), "{name}: {stop:?}");
        }
    }

    /// Fix round 2: right after an operator character, `/` is part of an
    /// s-expression operator, not a comment (`ion-rs`: `(+// [])` is
    /// `('+//' [])`). Read as a comment, the long string after it would hide
    /// the nesting inside it and the closing brackets before it would lower
    /// the count -- a scan behind the reader.
    #[test]
    fn a_slash_after_an_operator_is_an_operator() {
        let (rows, stop) = read_values(cursor("(a+// [c]) (a / [b])"), 10);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(
            rows,
            vec![json!(["a", "+//", ["c"]]), json!(["a", "/", ["b"]])]
        );
        let hidden = format!(
            "(+// ''' \n ]]] ''' {}{})",
            "[".repeat(MAX_DEPTH),
            "]".repeat(MAX_DEPTH)
        );
        let (_, stop) = read_values(cursor(hidden), 10);
        assert!(matches!(stop, Stop::TooDeep(_)), "{stop:?}");
    }

    /// Fix round 2: a binary length that runs past its container is refused
    /// before the reader sees it. `ion-rs` subtracts an annotation wrapper's
    /// lengths unchecked (`binary_buffer.rs:408`), which panics in a debug
    /// build and wraps in release.
    #[test]
    fn binary_lengths_past_their_container_are_refused() {
        for (name, tail) in [
            (
                "annotations longer than the wrapper",
                &[0xE3, 0x85, 0x81, 0x21, 0x05][..],
            ),
            ("no annotations", &[0xE3, 0x80, 0x21, 0x05][..]),
            (
                "a value past its list",
                &[0xB2, 0xE5, 0x81, 0x84, 0x21, 0x05][..],
            ),
            (
                "a string past its struct",
                &[0xD2, 0x84, 0x85, b'a', b'b'][..],
            ),
        ] {
            let mut bytes = vec![0xE0, 0x01, 0x00, 0xEA];
            bytes.extend(tail);
            let (rows, stop) = read_values(cursor(bytes), 10);
            assert!(rows.is_empty(), "{name}");
            assert!(
                matches!(&stop, Stop::Failed(m) if m.contains("binary Ion")),
                "{name}: {stop:?}"
            );
        }
        // Control: a well-formed annotated value inside a list reads.
        let ok = vec![0xE0, 0x01, 0x00, 0xEA, 0xB5, 0xE4, 0x81, 0x84, 0x21, 0x05];
        let (rows, stop) = read_values(cursor(ok), 10);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows, vec![json!([5])]);
    }

    /// A reader that hands out at most `chunk` bytes a call, as the store does.
    struct Chunky(std::io::Cursor<Vec<u8>>, usize);
    impl Read for Chunky {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.1);
            self.0.read(&mut buf[..n])
        }
    }

    /// Fix round 1, I2: one big value fed 16 KiB at a time was re-parsed after
    /// every fill (quadratic: 4 MiB took 79 s in the review). Each fill now
    /// fills the reader's whole buffer, which doubles.
    #[test]
    fn a_big_value_from_a_chunked_reader_reads_in_linear_time() {
        let mut list = String::from("[");
        while list.len() < 8 << 20 {
            list.push_str("1,");
        }
        list.push(']');
        let started = std::time::Instant::now();
        let (rows, stop) = read_values(Chunky(cursor(list), 16 << 10), 1);
        let took = started.elapsed();
        println!("8 MiB in one value, 16 KiB a read: {took:?}");
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows[0].as_array().unwrap().len(), 4 << 20);
        assert!(
            took < std::time::Duration::from_secs(TIME_BOUND_SECS),
            "8 MiB took {took:?}"
        );
    }

    #[test]
    fn scalars_lobs_annotations_and_special_floats() {
        let (rows, _) = values_to_json(
            &br#"null.int sym "s" {{aGVsbG8=}} {{"clob"}} a::b::5 nan +inf -inf 2025T 2025-12T 2025-12-03T 2025-12-03T12:00Z 2025-12-03T12:00:00-00:00 99999999999999999999999 -7 (1 2)"#[..],
            100,
        )
        .unwrap();
        assert_eq!(
            Value::Array(rows),
            json!([
                null,
                "sym",
                "s",
                "aGVsbG8=",
                "Y2xvYg==",
                5,
                "NaN",
                "Infinity",
                "-Infinity",
                "2025T",
                "2025-12T",
                "2025-12-03",
                "2025-12-03T12:00Z",
                "2025-12-03T12:00:00-00:00",
                serde_json::from_str::<Value>("99999999999999999999999").unwrap(),
                -7,
                [1, 2]
            ])
        );
    }
}
