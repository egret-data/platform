//! The Ion READER of the execution file API's preview (W9a spec §8), and of
//! a `Loop` over a file (§9.3).
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
//!
//! A `Loop` over a file does NOT use this table: Kestra iterates what
//! `FileSerde.read` made of each value, which differs for decimals, floats,
//! timestamps and `java.time`-annotated strings -- see `loop_value`.

use std::io::Read;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Why reading stopped.
#[derive(Debug)]
pub enum Stop {
    /// `max` values were read and another follows.
    Full,
    /// The input ended after the last value read.
    End,
    /// The input is not valid Ion, or the reader failed, after the values read.
    Failed(String),
    /// A container nests deeper than [`MAX_DEPTH`]: refused before the Ion
    /// reader saw the byte that opened it.
    TooDeep(String),
    /// A top-level value is longer than the reader's cap (see
    /// [`for_each_value`]): refused before the Ion reader saw the byte that
    /// crossed it.
    TooLarge(String),
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
    bytes: impl Read + Send,
    max: usize,
) -> Result<(Vec<Value>, bool /*truncated*/), String> {
    match read_values(bytes, max) {
        (rows, Stop::Full) => Ok((rows, true)),
        (rows, Stop::End) => Ok((rows, false)),
        (_, Stop::Failed(e) | Stop::TooDeep(e) | Stop::TooLarge(e) | Stop::Crashed(e)) => Err(e),
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
/// - **Every item is seen.** It reads with `ion-rs`'s `SystemReader`, so a
///   version marker and a local symbol table are items here too (consumed,
///   never yielded), and the symbol table is charged after each of them --
///   see [`for_each_value`].
/// - Reads one value past `max` only to learn whether more follow; any
///   outcome of that read (another value, an error, a refusal) is `Full`.
/// - A reader that fails mid-value (the preview's byte budget does, on
///   purpose) never yields that value cut short: the Ion reader asks for more
///   bytes before it ends a token, and gets the error.
pub(crate) fn read_values<R: Read + Send>(bytes: R, max: usize) -> (Vec<Value>, Stop) {
    read_values_capped(bytes, max, u64::MAX)
}

/// [`read_values`] under `cap` (`workflow.storage.max_read_bytes`): the
/// bounds of [`for_each_value`] -- the preview's.
pub fn read_values_capped<R: Read + Send>(bytes: R, max: usize, cap: u64) -> (Vec<Value>, Stop) {
    read_projected(bytes, max, cap, to_json)
}

/// Each top-level value of `bytes` as the string a Kestra `Loop` over that
/// file iterates (`FlowableUtils.ionValueToString`), or why it cannot be
/// one (a `null` record, a `java.time` text that does not parse), through
/// the same reader as [`read_values`] -- see
/// [`loop_value`] for the projection and why it is not the preview's.
pub(crate) fn loop_values<R: Read + Send>(
    bytes: R,
    max: usize,
    cap: u64,
) -> (Vec<Result<String, String>>, Stop) {
    read_projected(bytes, max, cap, loop_value)
}

/// [`read_values`]'s reader with the projection of each top-level value as a
/// parameter: the preview projects with [`to_json`], a `Loop` over a file
/// with [`loop_value`]. It is [`for_each_value`] collecting up to `max`
/// values: the ONE place the Ion reader runs (depth scan, its own thread,
/// linear-time fills).
fn read_projected<R, T>(
    bytes: R,
    max: usize,
    cap: u64,
    project: fn(&ion_rs::Element) -> T,
) -> (Vec<T>, Stop)
where
    R: Read + Send,
    T: Send,
{
    let mut rows = Vec::new();
    let stop = for_each_value(bytes, cap, |element| {
        if rows.len() == max {
            return ControlFlow::Break(());
        }
        rows.push(project(element));
        ControlFlow::Continue(())
    });
    match stop {
        // As before the sink: a reader that crashed hands back nothing.
        Stop::Crashed(why) => (Vec::new(), Stop::Crashed(why)),
        // Anything read after `max` values -- another value, an error, a
        // refusal -- is `Full`: the caller asked for no more.
        Stop::Failed(_) | Stop::TooDeep(_) | Stop::TooLarge(_) if rows.len() == max => {
            (rows, Stop::Full)
        }
        stop => (rows, stop),
    }
}

/// Every top-level value of `bytes`, handed to `sink` in file order as the
/// reader yields it -- **the one way into the Ion reader**: the preview and
/// a `Loop` over a file come through it by [`read_projected`], and W9a Task
/// 12's item tasks (`FilterItems`, `DeduplicateItems`, `Split` of a binary
/// Ion file) stream a file of any size through it, one value at a time,
/// holding none of the values it has passed on.
///
/// `sink` runs on the reader's own thread (see [`read_values`] for why it
/// has one), which is a scoped thread, so it may borrow from the caller.
/// It answers `Break` to stop reading: the answer is then [`Stop::Full`].
/// Otherwise the answer is how the input ended: [`Stop::End`], or why it is
/// not Ion past the values already handed over.
///
/// **What `max_value_bytes` (the cap, `workflow.storage.max_read_bytes`)
/// bounds** (W9a Task 12 fix rounds 1-4; `tests/ion_memory.rs` measures
/// each bound with a counting allocator -- peak live heap, not RSS). Each
/// refusal is [`Stop::TooLarge`] naming the cap; preview, `Loop` and the
/// item tasks all pass it.
///
/// - **Bytes.** Every byte handed to the reader counts, and the count
///   resets only when the reader yields a top-level value; the byte that
///   crosses the cap is never handed over. The reader's buffer (4 KiB,
///   doubled to fit) then holds under twice the cap of data, in under four
///   times the cap. A value is counted with up to one buffer of read-ahead,
///   so a file whose largest value is under about a third of the cap always
///   reads.
/// - **Values.** The depth scan also counts the values a top-level value can
///   hold -- exactly in binary, over-counted in text: every byte that can
///   start one (a timestamp counts several), and under an s-expression at
///   ANY depth (a comment's bracket opens a level the scan never closes,
///   so the innermost level alone could hide it) EVERY identifier byte,
///   since `ion-rs` splits a run there at each `$<digits>` symbol ID where
///   a list or struct refuses the adjacency (the split rule is never
///   mirrored: mirroring could under-count) -- and
///   a top-level value of more than `cap / ELEMENT_BYTES` is refused while
///   it is scanned, before the reader holds it. **Measured at the budget:**
///   33.8 MB under the 10 MiB cap for an s-expression of five million `$1`
///   (3.9 GB unbounded), 16.9 MB under a 4 MiB cap for a text list of a
///   million zeros (481 MB unbounded) -- the text reader's child cache,
///   about 206-270 bytes a child, plus the `Element`s, under the bound of
///   about 8 × the cap (see [`ELEMENT_BYTES`]). The count starts again at
///   the top level between values (binary: every container closed; text: a
///   top-level separator or closing bracket) and at each yield. The price
///   in text: one top-level value holding more than `cap / 64` values, or
///   more than `cap / 64` identifier bytes under an s-expression (about
///   160 KB at the default), is refused.
/// - **Symbols.** After EVERY item the reader consumes -- a version marker,
///   a symbol table, a value -- what its symbol table ADDED since the last
///   item is charged, its text plus [`SYMBOL_OVERHEAD`] a symbol, plus the
///   symbols of a table just read and not yet applied, against the cap
///   ([`SymbolCharge`]); a table that shrank (a version marker, a non-append
///   table) is counted again from nothing. So the table the reader holds
///   before a refusal is at most the cap plus one table, and one table is
///   at most `cap / 64` symbols by the value budget. **Measured:** 63
///   back-to-back tables of 163 000 symbols under the 10 MiB cap (each
///   under the value budget, the file under the byte cap) are refused at the
///   second at a 38 MB peak, where one such table reads holding 23 MB;
///   charged after values only (round 3) the same file peaked at 426 MB
///   (the reviewer's 494 MB in release). Kestra's `FileSerde` (a marker and
///   a table per row) and `ionfmt` text (no table) are never charged past a
///   row's symbols.
/// - **Not bounded by these:** one value's string or blob is held whole, up
///   to the byte cap; an annotation sequence reaches the count reset at a
///   top-level separator between its annotations, which `ion-rs` caps at 65
///   KB on its own.
pub(crate) fn for_each_value<R, F>(bytes: R, max_value_bytes: u64, sink: F) -> Stop
where
    R: Read + Send,
    F: FnMut(&ion_rs::Element) -> ControlFlow<()> + Send,
{
    std::thread::scope(|scope| {
        let spawned = std::thread::Builder::new()
            .name("egret-ion-reader".into())
            .stack_size(READER_STACK_BYTES)
            .spawn_scoped(scope, move || read_guarded(bytes, max_value_bytes, sink));
        match spawned {
            Err(e) => Stop::Crashed(format!("the Ion reader's thread did not start: {e}")),
            // Joined here, so a panic is this answer and not the scope's.
            Ok(handle) => handle.join().unwrap_or_else(|panic| {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "no message".into());
                Stop::Crashed(format!("the Ion reader panicked: {why}"))
            }),
        }
    })
}

/// The stack of the reader's thread: `MAX_DEPTH` levels need well under 1 MiB
/// in a debug build.
const READER_STACK_BYTES: usize = 16 << 20;

fn read_guarded<R: Read>(
    bytes: R,
    max_value_bytes: u64,
    mut sink: impl FnMut(&ion_rs::Element) -> ControlFlow<()>,
) -> Stop {
    let trip: Arc<Mutex<Option<Trip>>> = Arc::default();
    let since_yield = Arc::new(AtomicU64::new(0));
    let values_since = Arc::new(AtomicU64::new(0));
    let guarded = FillWhole {
        inner: DepthGuard {
            inner: bytes,
            scan: Scan::Undecided,
            max_value_bytes,
            since_yield: since_yield.clone(),
            max_values: max_value_bytes / ELEMENT_BYTES,
            values_since: values_since.clone(),
            trip: None,
            shared: trip.clone(),
        },
        pending: None,
    };
    // Capacity 0: every read goes straight through to `FillWhole`, whole.
    let input = std::io::BufReader::with_capacity(0, guarded);
    let tripped = || trip.lock().ok().and_then(|mut g| g.take());
    let failed = |e: ion_rs::IonError| match tripped() {
        Some(t) => t.into_stop(),
        None => Stop::Failed(e.to_string()),
    };
    // The SYSTEM reader (fix round 4): every top-level item it consumes is
    // seen here -- a version marker, a local symbol table, a value -- where
    // `Reader::next` is this same loop skipping all but the values
    // (`ion-rs` 1.0.1 `lazy/reader.rs:101`, `expanded/mod.rs:598`).
    let mut reader = ion_rs::SystemReader::new(ion_rs::AnyEncoding, input);
    // The system symbol table the reader starts with is never charged: it
    // is the same fixed handful for every input, and charging it made a
    // small cap refuse a value with no symbol at all (W9c Task 2 review I2).
    let mut symbols = SymbolCharge::new(reader.symbol_table().len());
    loop {
        use ion_rs::SystemStreamItem as Item;
        let element = match reader.next_item() {
            Err(e) => return failed(e),
            Ok(Item::EndOfStream(_)) => return Stop::End,
            // The value as `Reader::next` would have yielded it, built the
            // same way: its table was charged as pending after the item
            // that defined it (below), so this build is of a value whose
            // symbols were already found to fit.
            Ok(Item::Value(value)) => match ion_rs::Element::try_from(value) {
                Ok(element) => Some(element),
                Err(e) => return failed(e),
            },
            // A version marker, a symbol table, or (Ion 1.1) an encoding
            // directive: consumed by the reader, never yielded.
            Ok(_) => None,
        };
        if element.is_some() {
            // The ONLY reset of the byte count: ion-rs has read a whole
            // top-level VALUE (see `for_each_value`); a symbol table between
            // two values is counted with the value after it.
            since_yield.store(0, Ordering::SeqCst);
            values_since.store(0, Ordering::SeqCst);
        }
        // After EVERY item: what the reader's symbol table holds, by what
        // it added since the last item, plus what the item just read left
        // PENDING (a symbol table is applied only as the next item is read,
        // `expanded/mod.rs:between_top_level_expressions`) -- so a run of
        // tables is refused at the one that would not fit, before the
        // reader reads on.
        let pending = reader.pending_context_changes();
        if let Err(why) = symbols.charge(
            reader.symbol_table(),
            pending
                .local_symbols()
                .iter()
                .chain(pending.imported_symbols()),
            max_value_bytes,
        ) {
            return Stop::TooLarge(why);
        }
        if let Some(element) = element {
            if sink(&element).is_break() {
                return Stop::Full;
            }
        }
    }
}

/// How many bytes of `workflow.storage.max_read_bytes` one value inside a
/// top-level value costs (W9a Task 12 fix round 3): a top-level value of
/// more than `cap / ELEMENT_BYTES` values (itself, every nested one, every
/// annotation) is refused while its bytes are scanned, before `ion-rs` sees
/// the byte that crosses it -- the text reader caches every child it
/// matches (measured: about 270 bytes a child, retries included) and an
/// `Element` is 128 bytes in a `Vec` that doubles. So a value at the budget
/// holds at most about `cap / 64 × (270 + 256)` ≈ 8 × the cap: about 80 MiB
/// at the 10 MiB default, where a 10 MB text list of `0`s took 1.8 GB
/// unbounded (the review's probe).
pub(crate) const ELEMENT_BYTES: u64 = 64;

/// What one symbol the reader's symbol table holds costs beyond its text:
/// its `Symbol`, its `Arc`, its place in the table's `Vec` and map.
pub(crate) const SYMBOL_OVERHEAD: u64 = 64;

/// What the reader's symbol table costs, charged after EVERY item the
/// reader consumes (fix round 4; round 3 charged after each value only) by
/// what the table ADDED since the last item (its text plus
/// [`SYMBOL_OVERHEAD`] each) -- counted again from nothing when the table
/// shrank (a version marker, a symbol table that does not append), since
/// the old one is gone -- plus what the item left PENDING: `ion-rs` applies
/// a symbol table to its context only as it reads the next item, so right
/// after the table's own item its symbols are `pending_context_changes()`,
/// and charging them there refuses a table that will not fit before the
/// reader grows by it and reads on. The table the reader holds before a
/// refusal is therefore at most the cap plus one table, and one table is
/// bounded by the value budget ([`ELEMENT_BYTES`]: at most `cap / 64`
/// symbols). Charged after values only, back-to-back tables were seen only
/// at the next value's yield (the reviewer measured 63 tables held at once,
/// 494 MB, under a 10 MiB cap).
struct SymbolCharge {
    /// How many symbols of the table are the system table's, never charged.
    system: usize,
    len: usize,
    held: u64,
}

impl SymbolCharge {
    /// `system`: the length of the table the reader starts with (Ion 1.0's
    /// system symbols), which is never charged.
    fn new(system: usize) -> Self {
        Self {
            system,
            len: system,
            held: 0,
        }
    }

    /// `pending`: the symbols the reader has read but not yet applied (its
    /// `pending_context_changes()`, a type `ion-rs` does not export).
    fn charge<'s>(
        &mut self,
        table: &ion_rs::SymbolTable,
        pending: impl Iterator<Item = &'s ion_rs::Symbol>,
        cap: u64,
    ) -> Result<(), String> {
        let cost =
            |symbol: &ion_rs::Symbol| symbol.text().map_or(0, str::len) as u64 + SYMBOL_OVERHEAD;
        let symbols = table.symbols();
        if symbols.len() < self.len {
            // A version marker: the table is the system table again.
            self.len = self.system.min(symbols.len());
            self.held = 0;
        }
        for symbol in &symbols[self.len..] {
            self.held = self.held.saturating_add(cost(symbol));
        }
        self.len = symbols.len();
        let (pending_symbols, pending_cost) = pending.fold((0usize, 0u64), |(n, sum), symbol| {
            (n + 1, sum.saturating_add(cost(symbol)))
        });
        if self.held.saturating_add(pending_cost) > cap {
            return Err(format!(
                "the file's symbol tables define more symbols than workflow.storage.max_read_bytes \
                 ({cap} bytes) holds ({} symbols, {SYMBOL_OVERHEAD} bytes each over their text): \
                 egret reads only files whose symbols fit in that size",
                self.len + pending_symbols
            ));
        }
        Ok(())
    }
}

/// Why the [`DepthGuard`] stopped handing bytes over.
#[derive(Clone, Debug)]
enum Trip {
    TooDeep(String),
    /// A top-level value longer than the cap.
    TooLarge(String),
    /// Binary Ion the scan cannot follow (a reserved type code, an overlong
    /// length): refused rather than passed on unscanned.
    Malformed(String),
}

impl Trip {
    fn message(&self) -> &str {
        match self {
            Trip::TooDeep(m) | Trip::TooLarge(m) | Trip::Malformed(m) => m,
        }
    }
    fn into_stop(self) -> Stop {
        match self {
            Trip::TooDeep(m) => Stop::TooDeep(m),
            Trip::TooLarge(m) => Stop::TooLarge(m),
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
    /// The most bytes handed on between two values the reader yields.
    max_value_bytes: u64,
    /// The bytes handed on since the reader last yielded a top-level value
    /// (reset by `read_guarded`, never here).
    since_yield: Arc<AtomicU64>,
    /// The most values (the scan's [`Count`]) one top-level value may hold.
    max_values: u64,
    /// The values counted since the scan last reached the top level between
    /// values, or the reader last yielded one (reset by `read_guarded`).
    values_since: Arc<AtomicU64>,
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
            let fed = self.scan.feed(*b).and_then(|()| self.count());
            if let Err(t) = fed {
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

impl<R> DepthGuard<R> {
    /// Count one byte handed on: the byte that takes the bytes since the
    /// reader's last yield past `max_value_bytes` trips.
    fn count(&mut self) -> Result<(), Trip> {
        let counted = self.scan.take_count();
        if counted.values > 0 {
            let values = self
                .values_since
                .fetch_add(counted.values, Ordering::SeqCst)
                .saturating_add(counted.values);
            if values > self.max_values {
                return Err(Trip::TooLarge(format!(
                    "a top-level value holds more than {} values (one per {ELEMENT_BYTES} bytes \
                     of workflow.storage.max_read_bytes, {} bytes): a value is held whole to be \
                     read, so values are read only up to that size",
                    self.max_values, self.max_value_bytes
                )));
            }
        }
        if counted.top {
            self.values_since.store(0, Ordering::SeqCst);
        }
        let since = self.since_yield.fetch_add(1, Ordering::SeqCst) + 1;
        if since > self.max_value_bytes {
            return Err(Trip::TooLarge(format!(
                "a top-level value is longer than workflow.storage.max_read_bytes ({} bytes): \
                 a value is held whole to be read, so values are read only up to that size",
                self.max_value_bytes
            )));
        }
        Ok(())
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
    /// The values counted since the last call, and whether the scan is at
    /// the top level between values (binary: exactly, every container
    /// closed; text: at a top-level separator or closing bracket).
    fn take_count(&mut self) -> Count {
        match self {
            Scan::Text(t) => std::mem::take(&mut t.count),
            Scan::Binary(s) => {
                let mut count = std::mem::take(&mut s.count);
                count.top = matches!(s.state, Binary::Item)
                    && s.stack.first().is_none_or(|f| f.end <= s.pos);
                count
            }
            Scan::Undecided => Count::default(),
        }
    }

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
    /// See [`Count`].
    count: Count,
    /// The last byte scanned in `Normal` was an identifier character.
    in_word: bool,
    /// Bit `i` set: the container open at level `i + 1` is an s-expression
    /// (`MAX_DEPTH` levels fit; the bits above `depth` are stale).
    sexps: u128,
}

/// What a scan reports for the ELEMENT budget (W9a Task 12 fix round 3):
/// how many values it has seen start -- an OVER-count, never an under-count
/// of what the reader builds -- and whether the top level was reached
/// between values, where the count may start again.
#[derive(Default)]
struct Count {
    values: u64,
    top: bool,
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
    /// One level deeper, opened by `b` (`(` is an s-expression).
    fn open(&mut self, b: u8) -> Result<(), Trip> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(too_deep());
        }
        let bit = 1u128 << (self.depth - 1);
        if b == b'(' {
            self.sexps |= bit;
        } else {
            self.sexps &= !bit;
        }
        Ok(())
    }

    /// ANY open level is an s-expression (fix round 5; round 4 read the
    /// innermost only). A bracket inside a comment opens a level the scan
    /// never closes -- a phantom on top, whose bit `open` clears -- so the
    /// innermost bit could hide a real s-expression (`( /*[*/ $1$2$3 )`). A
    /// real s-expression's bit sits at a level the depth cannot drop below
    /// while it is open, since only real closers in `Normal` decrement, and
    /// `open` writes every level it enters, so no stale bit under `depth`
    /// is read. Identifiers in a list or struct nested inside an
    /// s-expression are over-counted: accepted.
    fn in_sexp(&self) -> bool {
        self.sexps & ((1u128 << self.depth) - 1) != 0
    }

    fn normal(&mut self, b: u8) -> Result<Text, Trip> {
        let after_operator = self.after_operator;
        self.after_operator = false;
        // The element count: every byte that can start a value -- each run
        // of identifier characters once, every other byte (an operator, a
        // bracket, a quote) once -- outside strings, symbols, lobs and
        // comments. A value is at least one such byte, so this is never
        // under what the reader builds; a timestamp or a decimal counts as
        // several. Under an s-expression, at any depth, EVERY identifier
        // byte counts (fix rounds 4-5): there `ion-rs` 1.0.1 splits a run
        // at each `$<digits>` symbol ID (`($1$2$3)` is three values;
        // `($1a$1a)` is `($ion a$1a)`), where a list or a struct refuses
        // the adjacency -- and the split rule is never mirrored, since
        // mirroring it could count UNDER what the reader builds. The price:
        // more than `cap / 64` identifier bytes (about 160 KB at the
        // default) under an s-expression in one top-level value is refused;
        // Kestra writes no s-expressions.
        let word = b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        if b.is_ascii_whitespace() || b == b',' {
            self.count.top |= self.depth == 0;
        } else if !(word && self.in_word && !self.in_sexp()) {
            self.count.values += 1;
        }
        self.in_word = word;
        Ok(match b {
            b'"' => Text::Str,
            b'\'' => Text::Quote1,
            b'/' if !after_operator => Text::Slash,
            b'{' => {
                self.open(b)?;
                Text::Brace
            }
            b'[' | b'(' => {
                self.open(b)?;
                Text::Normal
            }
            b']' | b')' | b'}' => {
                self.depth = self.depth.saturating_sub(1);
                self.count.top |= self.depth == 0;
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
                    self.open(b)?;
                    Text::LineComment
                }
                _ => Text::LineComment,
            },
            Text::BlockComment(star) => match b {
                b'/' if star => Text::Normal,
                b'[' | b'(' | b'{' => {
                    self.open(b)?;
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
    /// See [`Count`]: exact here -- one per type descriptor, and one per
    /// byte of an annotation wrapper's annotations.
    count: Count,
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
            Binary::TypeDescriptor => {
                // A version marker is not a value.
                if !(self.stack.is_empty() && b == 0xE0) {
                    self.count.values += 1;
                }
                self.type_descriptor(b)?
            }
            Binary::Length(acc, then) => match var_uint(acc, b)? {
                (len, true) => self.after_length(len, then)?,
                (acc, false) => Binary::Length(acc, then),
            },
            Binary::AnnotationsLength(acc) => match var_uint(acc, b)? {
                // At least one annotation, and room for the value after them.
                (len, true) => {
                    self.count.values = self.count.values.saturating_add(len);
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
        V::Timestamp(t) => Value::String(timestamp_text(t)),
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

/// A timestamp as [`to_json`] shows it: its Ion text, a day-precision date
/// without its `T` (as ion-java writes one), and UTC as `Z` (`-00:00`, an
/// unknown offset, stays). The KV store reads timestamps the same way.
pub(crate) fn timestamp_text(t: &ion_rs::Timestamp) -> String {
    let text = t.to_string();
    if t.precision() == ion_rs::TimestampPrecision::Day {
        text.trim_end_matches('T').to_string()
    } else if let Some(utc) = text.strip_suffix("+00:00") {
        format!("{utc}Z")
    } else {
        text
    }
}

/// Binary Ion 1.0's version marker, the first four bytes of every binary Ion
/// file: what Kestra's `IonStreamUtils.isIonBinary` reads to choose `Split`'s
/// Ion strategy over its text one (`StorageService.split`).
pub(crate) const BINARY_VERSION_MARKER: [u8; 4] = [0xE0, 0x01, 0x00, 0xEA];

/// One record of an item file as the variables Kestra's `FilterItems` and
/// `DeduplicateItems` render their expression with (W9a Task 12): the
/// record's fields, each a top-level name. Kestra reads the record into a
/// `Map` (`MAPPER.readValue(textIon, Map.class)`, `FilterItems.java`
/// `PebbleExpressionPredicate.extract`) and merges it OVER the run's
/// variables (`DefaultRunContext.render(String, Map)`,
/// `mergeWithNullableValues`), so a field `outputs` hides the task outputs,
/// and a `null` field is bound, as `null`, not dropped.
///
/// Each field's value is [`to_json`]'s, the preview's projection (an int, a
/// string, a bool as themselves; a decimal as a double; a timestamp as its
/// Ion text). Only ints and strings were measured (`w9a-storage.json`
/// `core_storage`'s `dedup` and `filter*` runs); what Kestra's mapper makes
/// of the other types inside a Pebble render was not. A field named twice
/// keeps its first place and its last value, as a `LinkedHashMap` does.
///
/// `Err` for a record that is not a struct: Kestra's `convertValue(…,
/// Map.class)` throws for it, failing the task whatever
/// `errorOrNullBehavior` says.
pub(crate) fn record_binding(
    element: &ion_rs::Element,
) -> Result<serde_json::Map<String, Value>, String> {
    let ion_rs::Value::Struct(st) = element.value() else {
        return Err(format!(
            "it is an Ion {}, not a struct, so it has no fields to bind",
            element.ion_type()
        ));
    };
    let mut map = serde_json::Map::new();
    for (name, value) in st.fields() {
        let json = if value.is_null() {
            Value::Null
        } else {
            to_json(value)
        };
        map.insert(name.text().unwrap_or("").to_string(), json);
    }
    Ok(map)
}

/// Kestra's message for a `null` record of a `Loop`'s values file
/// (`FlowableUtils.ionValueToString`).
pub const LOOP_NULL_RECORD: &str = "Found a null value in the ION file";

/// One top-level value as a Kestra `Loop` over its file iterates it:
/// `FlowableUtils.ionValueToString` of what `FileSerde.read` made of it --
/// a `String` as itself, a `Number`'s `toString()`, `null` refused
/// ([`LOOP_NULL_RECORD`]), anything else Kestra's JSON of it
/// (`egret_expected.loop_value_to_string`: Kestra's own method, run on the
/// artifact's jar in its container's zone).
///
/// **Not the preview's projection** ([`to_json`]): Kestra's preview shows a
/// decimal as a double and a timestamp as its Ion text, while `FileSerde`
/// makes a decimal a `BigDecimal`, a float a `Double` and a timestamp a
/// `java.time` value. So, here only:
///
/// - a decimal is [`java_big_decimal`] (`12.50` keeps its scale, `1d3` is
///   `1E+3`), a float Java's `Double.toString` (`1.0E20`);
/// - a timestamp is [`kestra_timestamp`], and a string annotated with a
///   `java.time` class name (what a JDBC plugin's STORE writes,
///   `LocalDateTime::"…"`) is [`java_time`] -- JSON-quoted at the top level,
///   because neither is a `String`; one that does not parse is an `Err`, as
///   `FileSerde.read` throws;
/// - a blob or clob at the top level is JSON-quoted base64 (a `byte[]`).
///
/// Still divergent, named by `LOOP_VALUE_DIVERGENT` in the tests: a date
/// before the Gregorian cutover, and a value with any other annotation,
/// which Kestra cannot serialise (it fails) and egret reads without it.
fn loop_value(element: &ion_rs::Element) -> Result<String, String> {
    use ion_rs::Value as V;
    if let Some(typed) = java_time_annotated(element) {
        return Ok(Value::String(typed?).to_string());
    }
    match element.value() {
        V::Null(_) => Err(LOOP_NULL_RECORD.to_string()),
        V::String(s) => Ok(s.text().to_string()),
        V::Symbol(s) => Ok(s.text().unwrap_or("").to_string()),
        V::Int(i) => Ok(i.to_string()),
        V::Decimal(d) => Ok(java_big_decimal(d)),
        V::Float(f) => {
            let mut out = String::new();
            crate::runners::env::write_java_double(&mut out, *f);
            Ok(out)
        }
        _ => loop_json(element),
    }
}

/// The JSON Kestra's mapper writes for a value `FileSerde` read: [`to_json`]
/// but for a decimal (a `BigDecimal`'s `toString()` as the number token, so
/// `{d:12.50}` is `{"d":12.50}` and `[1d3]` is `[1E+3]`), a finite float
/// (Java's `Double.toString`; NaN and the infinities stay strings), a
/// timestamp ([`kestra_timestamp`]) and a `java.time`-annotated string
/// ([`java_time`]), at every depth.
///
/// Written as TEXT, not built as a [`Value`]: `serde_json`'s number keeps
/// `12.50` but lowercases `1E+3`, which Kestra writes upper-case; a string
/// is escaped as Jackson escapes it (`\u001F`, upper case,
/// `runners::env::write_json_string`). A struct
/// field named twice keeps its first place and its last value, as a
/// `LinkedHashMap` (and [`to_json`]'s map) does; a field whose value is any
/// `null` is dropped. The depth is bounded as [`to_json`]'s is: the reader
/// never yields a value deeper than [`MAX_DEPTH`].
fn loop_json(element: &ion_rs::Element) -> Result<String, String> {
    use ion_rs::Value as V;
    // Jackson's escapes: `serde_json`'s, with `\uXXXX` in upper case.
    let quoted = |s: &str| {
        let mut out = String::new();
        crate::runners::env::write_json_string(&mut out, s);
        out
    };
    if let Some(typed) = java_time_annotated(element) {
        return Ok(quoted(&typed?));
    }
    Ok(match element.value() {
        V::String(s) => quoted(s.text()),
        V::Symbol(s) => match s.text() {
            Some(t) => quoted(t),
            None => "null".to_string(),
        },
        V::Decimal(d) => java_big_decimal(d),
        V::Float(f) if f.is_finite() => {
            let mut out = String::new();
            crate::runners::env::write_java_double(&mut out, *f);
            out
        }
        V::Timestamp(t) => quoted(&kestra_timestamp(t)),
        V::List(seq) | V::SExp(seq) => {
            let items = seq
                .elements()
                .map(loop_json)
                .collect::<Result<Vec<String>, String>>()?;
            format!("[{}]", items.join(","))
        }
        V::Struct(st) => {
            let mut fields: Vec<(String, String)> = Vec::new();
            let mut at: std::collections::HashMap<String, usize> = Default::default();
            for (name, value) in st.fields() {
                if value.is_null() {
                    continue;
                }
                let name = name.text().unwrap_or("").to_string();
                let json = loop_json(value)?;
                match at.get(&name) {
                    Some(&i) => fields[i].1 = json,
                    None => {
                        at.insert(name.clone(), fields.len());
                        fields.push((name, json));
                    }
                }
            }
            let body: Vec<String> = fields
                .iter()
                .map(|(name, json)| format!("{}:{json}", quoted(name)))
                .collect();
            format!("{{{}}}", body.join(","))
        }
        _ => to_json(element).to_string(),
    })
}

/// `java.math.BigDecimal.toString()` of the Ion decimal `d`, as `FileSerde`
/// reads it (a `BigDecimal` has no negative zero, so `-0.0` is `0.0`): the
/// plain digits when the scale is not negative and the adjusted exponent is
/// at least -6, else one digit, the rest after a point, and `E` with a signed
/// exponent (`1d3` is `1E+3`, `0.0000001` is `1E-7`, `12345678d2` is
/// `1.2345678E+9`).
fn java_big_decimal(d: &ion_rs::Decimal) -> String {
    let coefficient = d.coefficient();
    let digits = coefficient.magnitude().to_string();
    let sign = if coefficient.is_negative() && !d.is_zero() {
        "-"
    } else {
        ""
    };
    let scale = -d.exponent();
    let len = digits.len() as i64;
    let adjusted = -scale + (len - 1);
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    if scale > 0 && adjusted >= -6 {
        let point = len - scale;
        return if point > 0 {
            let (int, frac) = digits.split_at(point as usize);
            format!("{sign}{int}.{frac}")
        } else {
            format!("{sign}0.{}{digits}", "0".repeat((-point) as usize))
        };
    }
    let (first, rest) = digits.split_at(1);
    let mantissa = if rest.is_empty() {
        first.to_string()
    } else {
        format!("{first}.{rest}")
    };
    match adjusted {
        0 => format!("{sign}{mantissa}"),
        e if e > 0 => format!("{sign}{mantissa}E+{e}"),
        e => format!("{sign}{mantissa}E{e}"),
    }
}

/// An Ion timestamp as Kestra's mapper writes what `FileSerde` read
/// (`io.kestra.core.serializers.ion.IonParser.getEmbeddedObject`, `javap -c`
/// of the artifact's jar; `egret_expected.loop_value_to_string`):
///
/// - the instant is `Timestamp.calendarValue().toInstant()`, so MILLISECOND
///   precision, the rest truncated (`.1239` is `.123`);
/// - with no offset (`-00:00`, or a date) or offset zero, an instant at
///   midnight UTC is a `LocalDate`: `2025-12-03`;
/// - otherwise the instant in UTC with six fraction digits:
///   `2025-12-03T12:00:00.123000Z` -- a non-zero offset makes a
///   `ZonedDateTime`, which the artifact's mapper writes in the JVM's zone,
///   and the artifact's container runs in `Etc/UTC` (measured).
fn kestra_timestamp(t: &ion_rs::Timestamp) -> String {
    let utc = t.to_utc();
    let millis = t.milliseconds();
    let (y, mo, d) = (utc.year(), utc.month(), utc.day());
    let (h, mi, s) = (utc.hour(), utc.minute(), utc.second());
    if matches!(t.offset(), None | Some(0)) && h == 0 && mi == 0 && s == 0 && millis == 0 {
        format!("{y:04}-{mo:02}-{d:02}")
    } else {
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}000Z")
    }
}

/// The `java.time` class names Kestra's `IonParser.getEmbeddedObject` parses
/// an annotated string as (`javap -c` of the artifact's jar).
const JAVA_TIME_TYPES: [&str; 7] = [
    "Instant",
    "OffsetDateTime",
    "ZonedDateTime",
    "LocalDateTime",
    "LocalDate",
    "OffsetTime",
    "LocalTime",
];

/// `Some` when `element` is annotated with one of [`JAVA_TIME_TYPES`] (the
/// first annotation, as Jackson's type id): Kestra's JSON of the value
/// parsed as that class, or why it does not parse.
fn java_time_annotated(element: &ion_rs::Element) -> Option<Result<String, String>> {
    let class = element.annotations().first()?;
    if !JAVA_TIME_TYPES.contains(&class) {
        return None;
    }
    let text = match element.value() {
        ion_rs::Value::String(s) => s.text(),
        _ => {
            return Some(Err(format!(
                "a {class}-annotated Ion value is not a string"
            )))
        }
    };
    Some(
        java_time(class, text)
            .ok_or_else(|| format!("Text '{text}' could not be parsed as a {class}")),
    )
}

/// `text` parsed as the `java.time` class `class` (Java's ISO `parse`) and
/// written as Kestra's mapper writes it, the artifact's JVM zone being UTC
/// (`egret_expected.loop_value_to_string`, measured):
///
/// - `Instant`, `ZonedDateTime`: the instant in UTC, six fraction digits
///   (truncated), `Z` -- Kestra's own serialisers;
/// - `OffsetDateTime`: the instant in UTC, the fewest fraction digits, `Z`;
/// - `LocalDateTime`, `LocalTime`: as given, seconds always, the fewest
///   fraction digits (`.100` is `.1`);
/// - `LocalDate` as given; `OffsetTime` as given, its offset kept.
///
/// A `ZonedDateTime`'s `[region]` is skipped: its offset fixes the instant.
fn java_time(class: &str, text: &str) -> Option<String> {
    use chrono::{NaiveDateTime, Timelike as _};
    let micros6 = |dt: NaiveDateTime| {
        format!(
            "{}.{:06}Z",
            dt.format("%Y-%m-%dT%H:%M:%S"),
            dt.nanosecond() / 1_000
        )
    };
    match class {
        "LocalDate" => {
            let (date, rest) = iso_date(text)?;
            rest.is_empty().then(|| date.format("%Y-%m-%d").to_string())
        }
        "LocalTime" => {
            let (time, rest) = iso_time(text)?;
            rest.is_empty().then(|| fewest(time))
        }
        "OffsetTime" => {
            let (time, rest) = iso_time(text)?;
            let (offset, rest) = iso_offset(rest)?;
            rest.is_empty()
                .then(|| format!("{}{}", fewest(time), offset_text(offset)))
        }
        "LocalDateTime" => {
            let (dt, rest) = iso_date_time(text)?;
            rest.is_empty()
                .then(|| format!("{}T{}", dt.date().format("%Y-%m-%d"), fewest(dt.time())))
        }
        "OffsetDateTime" | "ZonedDateTime" | "Instant" => {
            let (dt, rest) = iso_date_time(text)?;
            let (offset, rest) = iso_offset(rest)?;
            let rest = match rest.strip_prefix('[') {
                // A region after the offset: `ZonedDateTime` only, and whole.
                Some(region) if class == "ZonedDateTime" => {
                    region.strip_suffix(']').filter(|r| !r.is_empty())?;
                    0
                }
                _ => rest.len(),
            };
            if rest != 0 {
                return None;
            }
            let utc = dt - chrono::Duration::seconds(i64::from(offset));
            Some(if class == "OffsetDateTime" {
                format!("{}T{}Z", utc.date().format("%Y-%m-%d"), fewest(utc.time()))
            } else {
                micros6(utc)
            })
        }
        _ => None,
    }
}

/// `HH:mm:ss` and the fewest fraction digits (none for a whole second), as
/// Java's `ISO_LOCAL_TIME` formats.
fn fewest(time: chrono::NaiveTime) -> String {
    use chrono::Timelike as _;
    let whole = time.format("%H:%M:%S").to_string();
    match time.nanosecond() {
        0 => whole,
        n => format!("{whole}.{}", format!("{n:09}").trim_end_matches('0')),
    }
}

/// `Z` for a zero offset, else `+HH:MM` (with `:SS` when it has seconds), as
/// Java's `ISO_OFFSET_TIME` formats.
fn offset_text(seconds: i32) -> String {
    if seconds == 0 {
        return "Z".into();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let a = seconds.unsigned_abs();
    let (h, m, s) = (a / 3600, a / 60 % 60, a % 60);
    if s == 0 {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!("{sign}{h:02}:{m:02}:{s:02}")
    }
}

/// `uuuu-MM-dd` (four-digit year) and what follows it.
fn iso_date(text: &str) -> Option<(chrono::NaiveDate, &str)> {
    fn digits(s: &str) -> Option<u32> {
        s.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| s.parse().ok())?
    }
    let (y, rest) = (text.get(..4)?, text.get(4..)?);
    let (m, rest) = (rest.strip_prefix('-')?.get(..2)?, rest.get(3..)?);
    let (d, rest) = (rest.strip_prefix('-')?.get(..2)?, rest.get(3..)?);
    Some((
        chrono::NaiveDate::from_ymd_opt(digits(y)? as i32, digits(m)?, digits(d)?)?,
        rest,
    ))
}

/// `HH:mm[:ss[.f…]]` (one to nine fraction digits) and what follows it.
fn iso_time(text: &str) -> Option<(chrono::NaiveTime, &str)> {
    let two = |s: &str| -> Option<u32> {
        (s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok())?
    };
    let h = two(text.get(..2)?)?;
    let rest = text.get(2..)?.strip_prefix(':')?;
    let m = two(rest.get(..2)?)?;
    let mut rest = rest.get(2..)?;
    let (mut s, mut nanos) = (0, 0);
    if let Some(after) = rest.strip_prefix(':') {
        s = two(after.get(..2)?)?;
        rest = after.get(2..)?;
        if let Some(frac) = rest.strip_prefix('.') {
            let n = frac.bytes().take_while(u8::is_ascii_digit).count();
            if !(1..=9).contains(&n) {
                return None;
            }
            nanos = format!("{:0<9}", &frac[..n]).parse().ok()?;
            rest = &frac[n..];
        }
    }
    Some((chrono::NaiveTime::from_hms_nano_opt(h, m, s, nanos)?, rest))
}

/// `uuuu-MM-ddTHH:mm[:ss[.f…]]` and what follows it.
fn iso_date_time(text: &str) -> Option<(chrono::NaiveDateTime, &str)> {
    let (date, rest) = iso_date(text)?;
    let (time, rest) = iso_time(rest.strip_prefix('T')?)?;
    Some((date.and_time(time), rest))
}

/// `Z` or `±HH:MM[:SS]`, in seconds, and what follows it.
fn iso_offset(text: &str) -> Option<(i32, &str)> {
    if let Some(rest) = text.strip_prefix('Z') {
        return Some((0, rest));
    }
    let sign = match text.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let (time, rest) = iso_time(&text[1..])?;
    if time.nanosecond() != 0 || time.hour() > 18 {
        return None;
    }
    use chrono::Timelike as _;
    Some((sign * time.num_seconds_from_midnight() as i32, rest))
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

    /// A hang guard only, per read in the linear-time test: 8 MiB took
    /// about 7 s linear and had not finished after 31 minutes quadratic; 4 MiB
    /// quadratic took 79 s. The test's verdict is its ratio.
    const HANG_GUARD_SECS: u64 = 300;
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

    // ---- W9a Task 12 fix round 1, I1: one value is held whole, so capped ----

    /// `head`, then `len` bytes of `fill`, then `tail`, made as they are read
    /// (a 32 MiB value costs no 32 MiB here); `pulled` counts what was read.
    struct Generated {
        head: Vec<u8>,
        fill: u8,
        len: u64,
        tail: Vec<u8>,
        at: u64,
        pulled: Arc<std::sync::atomic::AtomicU64>,
    }

    impl Generated {
        fn new(head: &[u8], fill: u8, len: u64, tail: &[u8]) -> Self {
            Self {
                head: head.to_vec(),
                fill,
                len,
                tail: tail.to_vec(),
                at: 0,
                pulled: Arc::default(),
            }
        }
    }

    impl Read for Generated {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let (h, l) = (self.head.len() as u64, self.len);
            let total = h + l + self.tail.len() as u64;
            let mut n = 0;
            while n < buf.len() && self.at < total {
                buf[n] = if self.at < h {
                    self.head[self.at as usize]
                } else if self.at < h + l {
                    self.fill
                } else {
                    self.tail[(self.at - h - l) as usize]
                };
                n += 1;
                self.at += 1;
            }
            self.pulled
                .fetch_add(n as u64, std::sync::atomic::Ordering::SeqCst);
            Ok(n)
        }
    }

    /// Every value `for_each_value` hands over under `cap`, as JSON, and why it stopped.
    fn capped(bytes: impl Read + Send, cap: u64) -> (Vec<Value>, Stop) {
        let mut rows = Vec::new();
        let stop = for_each_value(bytes, cap, |e| {
            rows.push(to_json(e));
            ControlFlow::Continue(())
        });
        (rows, stop)
    }

    /// A binary Ion string header of `len` bytes (type 8, VarUInt length).
    fn binary_string_header(len: u64) -> Vec<u8> {
        let mut groups = Vec::new();
        let mut n = len;
        while n > 0 || groups.is_empty() {
            groups.push((n & 0x7F) as u8);
            n >>= 7;
        }
        groups.reverse();
        *groups.last_mut().unwrap() |= 0x80;
        let mut out = vec![0x8E];
        out.extend(groups);
        out
    }

    /// **A value over the cap is refused naming it, and the reader never
    /// pulls the rest**: one 32 MiB string, text and binary, under a 1 MiB
    /// cap reads at most a few MiB and stops `TooLarge`; the value before it
    /// stands. Without the cap the reader pulls all 32 MiB and holds them.
    #[test]
    fn a_value_over_the_cap_is_refused_and_the_rest_never_read() {
        const CAP: u64 = 1 << 20;
        const BIG: u64 = 32 << 20;
        let text = Generated::new(b"{a:1}\n{s:\"", b'x', BIG, b"\"}");
        let mut binary_head = BINARY_VERSION_MARKER.to_vec();
        binary_head.extend([0x21, 0x07]); // the int 7
        binary_head.extend(binary_string_header(BIG));
        let binary = Generated::new(&binary_head, b'x', BIG, b"");
        for (what, input, first) in [
            ("text", text, json!({"a": 1})),
            ("binary", binary, json!(7)),
        ] {
            let pulled = input.pulled.clone();
            let (rows, stop) = capped(input, CAP);
            // The count and the first, not the rows: a failure must not
            // print a 32 MiB string.
            assert_eq!(rows.len(), 1, "{what}");
            assert_eq!(rows[0], first, "{what}");
            let Stop::TooLarge(why) = stop else {
                panic!("{what}: {stop:?}")
            };
            assert!(why.contains(&format!("{CAP} bytes")), "{what}: {why}");
            assert!(why.contains("max_read_bytes"), "{what}: {why}");
            let pulled = pulled.load(std::sync::atomic::Ordering::SeqCst);
            assert!(
                pulled < 4 * CAP,
                "{what}: pulled {pulled} bytes for a {CAP}-byte cap"
            );
        }
    }

    /// A binary Ion header of type `t` (the high nibble) and `len` bytes,
    /// in the long form: `tE` and a VarUInt length.
    fn binary_header(t: u8, len: usize) -> Vec<u8> {
        let mut out = binary_string_header(len as u64);
        out[0] = (t << 4) | 0x0E;
        out
    }

    /// A binary local symbol table appending one symbol of `len` bytes:
    /// `$ion_symbol_table::{imports: $ion_symbol_table, symbols: ["ss…"]}`.
    fn binary_append_lst(len: usize) -> Vec<u8> {
        let mut symbol = binary_header(8, len);
        symbol.extend(std::iter::repeat_n(b's', len));
        let mut list = binary_header(11, symbol.len());
        list.extend(symbol);
        // imports ($6): the symbol $3; symbols ($7): the list.
        let mut fields = vec![0x86, 0x71, 0x03, 0x87];
        fields.extend(list);
        let mut st = binary_header(13, fields.len());
        st.extend(fields);
        // One annotation, $3 (`$ion_symbol_table`).
        let mut wrapped = vec![0x81, 0x83];
        wrapped.extend(st);
        let mut out = binary_header(14, wrapped.len());
        out.extend(wrapped);
        out
    }

    /// The text of the same append: one symbol of `len` bytes.
    fn text_append_lst(len: usize) -> String {
        format!(
            "$ion_symbol_table::{{imports:$ion_symbol_table,symbols:[\"{}\"]}}\n",
            "s".repeat(len)
        )
    }

    const KIB: usize = 1 << 10;

    /// Fix round 2: the count resets only when the reader YIELDS a value,
    /// so ONE value built from parts each under the cap is counted whole --
    /// a chain of quoted annotations, each 12 KiB under a 16 KiB cap, is one
    /// value (the premise: uncapped, it reads as one `1`; `ion-rs` itself
    /// refuses an annotation sequence over 65 KB), refused; so are adjacent
    /// long strings.
    #[test]
    fn an_annotation_chain_is_one_value_and_counted_whole() {
        let part = format!("'{}'::", "a".repeat(12 * KIB));
        let chain = format!("{}1\n", part.repeat(4));
        let (rows, stop) = capped(cursor(chain.clone()), u64::MAX);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows, vec![json!(1)], "the premise: one value");
        let (rows, stop) = capped(cursor(chain), 16 * KIB as u64);
        assert!(rows.is_empty(), "the chain was read: {stop:?}");
        let Stop::TooLarge(why) = stop else {
            panic!("{stop:?}")
        };
        assert!(why.contains("max_read_bytes (16384 bytes)"), "{why}");
        let long = format!("'''{}''' ", "b".repeat(40 * KIB)).repeat(3);
        let (rows, stop) = capped(cursor(long.clone()), u64::MAX);
        assert_eq!(rows.len(), 1, "the premise: {stop:?}");
        let (_, stop) = capped(cursor(long), 64 * KIB as u64);
        assert!(matches!(stop, Stop::TooLarge(_)), "{stop:?}");
    }

    /// Fix round 2: local symbol tables are consumed without a yield, so a
    /// run of appends -- each 40 KiB, under a 64 KiB cap -- is counted whole
    /// and refused, text and binary (the premise: uncapped, the file is the
    /// one value after them).
    #[test]
    fn a_run_of_symbol_table_appends_is_counted_whole() {
        let text = format!("{}1\n", text_append_lst(40 * KIB).repeat(4));
        let mut binary = BINARY_VERSION_MARKER.to_vec();
        for _ in 0..4 {
            binary.extend(binary_append_lst(40 * KIB));
        }
        binary.extend([0x21, 0x01]);
        let mut read = Vec::new();
        for (what, bytes) in [("text", text.into_bytes()), ("binary", binary)] {
            let (rows, stop) = capped(cursor(bytes.clone()), u64::MAX);
            assert!(matches!(stop, Stop::End), "{what}: {stop:?}");
            assert_eq!(rows, vec![json!(1)], "{what}: the premise");
            let (rows, stop) = capped(cursor(bytes), 64 * KIB as u64);
            if !rows.is_empty() || !matches!(stop, Stop::TooLarge(_)) {
                read.push(format!("{what}: {rows:?} {stop:?}"));
            }
        }
        assert!(read.is_empty(), "read past the cap: {read:?}");
    }

    /// The control: the cap bounds what is read between two values, not the
    /// file. Under a 64 KiB cap, 1 MiB of small values reads (text and
    /// binary), and so does 1 MiB of 16 KiB values -- each value plus the
    /// reader's buffer (doubled to 32 KiB to fit it) is under the cap, the
    /// over-count margin. One value of twice the cap is refused.
    #[test]
    fn values_under_the_cap_read_however_large_the_file() {
        const CAP: u64 = 64 * KIB as u64;
        let small = "{a:1}\n".repeat(200_000);
        let mut binary = BINARY_VERSION_MARKER.to_vec();
        for _ in 0..250_000 {
            binary.extend([0xD3, 0x84, 0x21, 0x01]); // {name: 1} (`name` is $4)
        }
        let sixteen = format!("\"{}\"\n", "c".repeat(16 * KIB)).repeat(64);
        for (what, bytes, n) in [
            ("small text", small.into_bytes(), 200_000),
            ("small binary", binary, 250_000),
            ("16 KiB strings", sixteen.into_bytes(), 64),
        ] {
            let mut count = 0usize;
            let stop = for_each_value(cursor(bytes), CAP, |_| {
                count += 1;
                ControlFlow::Continue(())
            });
            assert!(matches!(stop, Stop::End), "{what}: {stop:?}");
            assert_eq!(count, n, "{what}");
        }
        let big = format!("1 \"{}\" 2", "d".repeat(2 * CAP as usize));
        let (rows, stop) = capped(cursor(big), CAP);
        assert_eq!(rows, vec![json!(1)]);
        assert!(matches!(stop, Stop::TooLarge(_)), "{stop:?}");
    }

    // ---- W9a Task 12 fix round 3: symbol tables and element counts --------

    /// A binary symbol table appending `k` symbols with no text (`null`, one
    /// byte each -- the cheapest symbol there is).
    fn binary_null_symbol_table(k: usize) -> Vec<u8> {
        let mut list = binary_header(11, k);
        list.extend(std::iter::repeat_n(0x0Fu8, k));
        let mut fields = vec![0x86, 0x71, 0x03, 0x87];
        fields.extend(list);
        let mut st = binary_header(13, fields.len());
        st.extend(fields);
        let mut wrapped = vec![0x81, 0x83];
        wrapped.extend(st);
        let mut out = binary_header(14, wrapped.len());
        out.extend(wrapped);
        out
    }

    /// [`binary_null_symbol_table`], then the int `i`.
    fn binary_null_symbols_then(k: usize, i: u8) -> Vec<u8> {
        let mut out = binary_null_symbol_table(k);
        out.extend([0x21, i]);
        out
    }

    /// (A) Append-mode symbol tables BETWEEN values -- each value resets the
    /// byte count -- are charged by what the reader's table grew: 50 tables
    /// of 1 000 symbols, text strings and binary nulls, under a 64 KiB cap
    /// (64 bytes a symbol) are refused naming the cap; uncapped, all 50
    /// values read (the premise).
    #[test]
    fn symbol_tables_between_values_are_charged_as_they_grow() {
        let symbols = vec!["\"a\""; 1000].join(",");
        let text: String = (0..50)
            .map(|i| {
                format!(
                    "$ion_symbol_table::{{imports:$ion_symbol_table,symbols:[{symbols}]}} {i}\n"
                )
            })
            .collect();
        let mut binary = BINARY_VERSION_MARKER.to_vec();
        for i in 0..50 {
            binary.extend(binary_null_symbols_then(1000, i));
        }
        for (what, bytes) in [("text", text.into_bytes()), ("binary", binary)] {
            let (rows, stop) = capped(cursor(bytes.clone()), u64::MAX);
            assert!(matches!(stop, Stop::End), "{what}: {stop:?}");
            assert_eq!(rows.len(), 50, "{what}: the premise");
            let (rows, stop) = capped(cursor(bytes), 64 * KIB as u64);
            assert!(rows.len() < 50, "{what}: {} values read", rows.len());
            let Stop::TooLarge(why) = stop else {
                panic!("{what}: {stop:?}")
            };
            assert!(why.contains("symbol"), "{what}: {why}");
            assert!(
                why.contains("max_read_bytes (65536 bytes)"),
                "{what}: {why}"
            );
        }
        // ONE table of 2 000 null symbols (a byte each) is refused by the
        // value budget (1 024) while it is scanned -- before the reader,
        // which holds a symbol in about 50 bytes, has it whole.
        let mut one = BINARY_VERSION_MARKER.to_vec();
        one.extend(binary_null_symbols_then(2000, 1));
        let (rows, stop) = capped(cursor(one), 64 * KIB as u64);
        assert!(rows.is_empty());
        let Stop::TooLarge(why) = stop else {
            panic!("{stop:?}")
        };
        assert!(why.contains("more than 1024 values"), "{why}");
    }

    /// The symbol charge counts what the table HOLDS: Kestra's `FileSerde`
    /// writes a version marker and a fresh symbol table per row, so the
    /// table never grows past one row's symbols -- 20 000 such rows read
    /// under the same 64 KiB cap that refuses the appends above.
    #[test]
    fn a_symbol_table_per_row_is_never_charged_twice() {
        let kestra = b64(
            "4AEA6umBg9aHtIFrgXbWioFiiyEC4AEA6umBg9aHtIFrgXbWioFhiyED4AEA6umBg9aHtIFrgXbWioFjiyEE",
        );
        let bytes = kestra.repeat(20_000 / 3 + 1);
        let mut count = 0usize;
        let stop = for_each_value(cursor(bytes), 64 * KIB as u64, |_| {
            count += 1;
            ControlFlow::Continue(())
        });
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(count, 3 * (20_000 / 3 + 1));
    }

    /// (B) A value of more elements than `cap / ELEMENT_BYTES` -- itself,
    /// every nested value, every annotation, over-counted by the scan (a
    /// bracket counts) -- is refused before the reader sees it whole, inside
    /// the byte cap; half the budget reads, text and binary.
    #[test]
    fn a_value_past_the_element_budget_is_refused() {
        let cap = 64 * KIB as u64;
        let budget = (cap / ELEMENT_BYTES) as usize; // 1024
        let list = |n: usize| format!("[{}]", vec!["0"; n].join(","));
        let (rows, stop) = capped(cursor(list(budget / 2).repeat(100)), cap);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows.len(), 100, "the count starts again at each value");
        let mut binary = BINARY_VERSION_MARKER.to_vec();
        for (n, ok) in [(budget / 2, true), (budget, false)] {
            let mut file = binary.clone();
            file.extend(binary_header(11, n));
            file.extend(std::iter::repeat_n(0x20u8, n));
            let (_, stop) = capped(cursor(file), cap);
            assert_eq!(matches!(stop, Stop::End), ok, "binary {n}: {stop:?}");
        }
        binary.clear();
        let (rows, stop) = capped(cursor(format!("1 {}", list(budget))), cap);
        assert_eq!(rows, vec![json!(1)]);
        let Stop::TooLarge(why) = stop else {
            panic!("{stop:?}")
        };
        assert!(why.contains("more than 1024 values"), "{why}");
        // Nested and annotated values count too.
        let nested = format!("{{a:{}}}", list(budget / 2)).repeat(1);
        let (_, stop) = capped(cursor(format!("[{nested},{nested}]")), cap);
        assert!(matches!(stop, Stop::TooLarge(_)), "{stop:?}");
        let annotated = format!("{}0", "x::".repeat(budget));
        let (_, stop) = capped(cursor(annotated), cap);
        assert!(matches!(stop, Stop::TooLarge(_)), "{stop:?}");
    }

    // ---- W9a Task 12 fix round 4: every item charged; sexp word bytes ----

    /// **(A)** Symbol tables BACK TO BACK, each under the value budget, are
    /// charged as the reader consumes each one -- not at the next user
    /// value, by which time the table held them all. 20 append-mode tables
    /// of 16 000 `null` symbols (16 KiB each, 320 KiB in all) under a 1 MiB
    /// cap: one table then a value reads (the premise); the run is refused
    /// naming the symbols, and the reader pulled under 128 KiB of the 320
    /// (before: all of it, the charge waiting for the value at the end).
    #[test]
    fn back_to_back_symbol_tables_are_charged_before_the_next_value() {
        const CAP: u64 = 1 << 20;
        let table = binary_null_symbol_table(16_000);
        let mut one = BINARY_VERSION_MARKER.to_vec();
        one.extend(&table);
        one.extend([0x21, 0x01]);
        let (rows, stop) = capped(cursor(one), CAP);
        assert!(matches!(stop, Stop::End), "the premise: {stop:?}");
        assert_eq!(rows, vec![json!(1)]);
        let mut many = BINARY_VERSION_MARKER.to_vec();
        for _ in 0..20 {
            many.extend(&table);
        }
        many.extend([0x21, 0x01]);
        let total = many.len() as u64;
        let input = Generated::new(&many, 0, 0, b"");
        let pulled = input.pulled.clone();
        let (rows, stop) = capped(input, CAP);
        assert!(rows.is_empty(), "{rows:?}");
        let Stop::TooLarge(why) = stop else {
            panic!("{stop:?}")
        };
        assert!(why.contains("symbol"), "{why}");
        assert!(why.contains("max_read_bytes (1048576 bytes)"), "{why}");
        let pulled = pulled.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            pulled < 128 * KIB as u64,
            "pulled {pulled} of {total} bytes: the tables were charged only at the value"
        );
    }

    /// **(B)** Inside an s-expression `ion-rs` splits a run of word
    /// characters at every `$<digits>` symbol ID (the premise: `($1$2$3)` is
    /// three symbols), so there EVERY word byte counts as a value -- a run
    /// of 600 `$1` (1 200 bytes, under a 64 KiB cap) is refused by the
    /// budget of 1 024, not built. Elsewhere a run counts once: the same run
    /// in a list is `ion-rs`'s own error (lists refuse adjacency), and an
    /// s-expression of ordinary tokens -- ints, symbols, timestamps, a struct
    /// and a list -- under the budget reads.
    #[test]
    fn a_sexp_counts_every_word_byte_as_a_value() {
        let cap = 64 * KIB as u64;
        let (rows, stop) = capped(cursor("($1$2$3)"), u64::MAX);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(
            rows,
            vec![json!(["$ion", "$ion_1_0", "$ion_symbol_table"])],
            "the premise: ion-rs splits the run"
        );
        let run = "$1".repeat(600);
        let (rows, stop) = capped(cursor(format!("({run})")), cap);
        assert!(rows.is_empty(), "{rows:?}");
        let Stop::TooLarge(why) = stop else {
            panic!("{stop:?}")
        };
        assert!(why.contains("more than 1024 values"), "{why}");
        let (rows, stop) = capped(cursor(format!("[{run}]")), cap);
        assert!(rows.is_empty(), "{rows:?}");
        assert!(
            matches!(stop, Stop::Failed(_)),
            "a list refuses adjacency: {stop:?}"
        );
        // The threshold: an s-expression's word bytes are its values (its
        // tokens were). 1 000 of them, in 250 symbols, read under the budget
        // of 1 024 (the `(` counts too); 1 032 are refused.
        let words = |n: usize| format!("({})", vec!["abcd"; n / 4].join(" "));
        let (rows, stop) = capped(cursor(words(1_000)), cap);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows.len(), 1);
        let (_, stop) = capped(cursor(words(1_032)), cap);
        assert!(matches!(stop, Stop::TooLarge(_)), "{stop:?}");
        let ordinary =
            "(1 22 333 abc d_e 2025-12-03T12:00:00.123Z {a:1,b:[x,y]} [1,2] \"s\" 'q')\n";
        let (rows, stop) = capped(cursor(ordinary.repeat(1_000)), cap);
        assert!(matches!(stop, Stop::End), "{stop:?}");
        assert_eq!(rows.len(), 1_000);
        assert_eq!(
            rows[0],
            json!([1, 22, 333, "abc", "d_e", "2025-12-03T12:00:00.123Z", {"a": 1, "b": ["x", "y"]}, [1, 2], "s", "q"])
        );
    }

    /// **(B) fix round 5:** a bracket inside a comment opens a level the
    /// scan never closes (so it is never behind `ion-rs`), and round 4 read
    /// only the innermost level's s-expression bit, so a comment could hide
    /// the s-expression from the word count. A word byte now counts while
    /// ANY open level is an s-expression: 600 `$1` after `/*[*/`, after
    /// `//{` and a newline, and in `[( /*[*/ … )]` are refused under a 64
    /// KiB cap (the premise: each reads uncapped as the split symbols), and
    /// the same comments before a list of 300 words still read.
    #[test]
    fn a_comment_bracket_cannot_hide_a_sexp() {
        let cap = 64 * KIB as u64;
        for (what, head, tail) in [
            ("block comment", "( /*[*/ ", " )"),
            ("line comment", "( //{\n ", " )"),
            ("nested in a list", "[( /*[*/ ", " )]"),
        ] {
            let small = format!("{head}$1$2{tail}");
            let (rows, stop) = capped(cursor(small), u64::MAX);
            assert!(matches!(stop, Stop::End), "{what}: {stop:?}");
            assert_eq!(rows.len(), 1, "{what}: the premise, ion-rs reads it");
            let text = rows[0].to_string();
            assert!(text.contains("$ion_1_0"), "{what}: not split: {text}");
            let (rows, stop) = capped(cursor(format!("{head}{}{tail}", "$1".repeat(600))), cap);
            assert!(rows.is_empty(), "{what}: {rows:?}");
            let Stop::TooLarge(why) = stop else {
                panic!("{what}: {stop:?}")
            };
            assert!(why.contains("more than 1024 values"), "{what}: {why}");
        }
        // The control: the same comments do not make a list a sexp -- 300
        // words (1 200 identifier bytes, over the budget if they counted
        // each) read as 300 values.
        let words = vec!["abcd"; 300].join(", ");
        for text in [
            format!("[ /*[*/ {words} ]"),
            format!("[ //{{\n {words} ]"),
            format!("{{ /*{{*/ a: [{words}] }}"),
        ] {
            let (rows, stop) = capped(cursor(text.clone()), cap);
            assert!(matches!(stop, Stop::End), "{text}: {stop:?}");
            assert_eq!(rows.len(), 1, "{text}");
        }
        // The price, pinned: a `(` inside a comment opens a phantom
        // s-expression level, under which the same list IS refused.
        let (_, stop) = capped(cursor(format!("/*(*/ [{words}]")), cap);
        assert!(matches!(stop, Stop::TooLarge(_)), "{stop:?}");
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
    ///
    /// W9a Task 10 fix round 1, I3: judged by a RATIO, not a wall clock (the
    /// 20 s bound passed in 8 s alone and failed in 22.7 s at load ~30). The
    /// same run reads 1 MiB and 4 MiB, each the best of two: linear is about
    /// 4x, quadratic about 16x; the bound is 8x. 1 and 4 rather than 2 and 8
    /// so the quadratic mutant can be SHOWN failing it -- at 8 MiB it had not
    /// finished after 31 minutes. The absolute ceiling is only a hang guard.
    #[test]
    fn a_big_value_from_a_chunked_reader_reads_in_linear_time() {
        let read = |bytes: usize| {
            let mut list = String::from("[");
            while list.len() < bytes {
                list.push_str("1,");
            }
            list.push(']');
            let started = std::time::Instant::now();
            let (rows, stop) = read_values(Chunky(cursor(list), 16 << 10), 1);
            let took = started.elapsed();
            assert!(matches!(stop, Stop::End), "{stop:?}");
            assert_eq!(rows[0].as_array().unwrap().len(), bytes / 2);
            assert!(
                took < std::time::Duration::from_secs(HANG_GUARD_SECS),
                "{bytes} bytes took {took:?}"
            );
            took
        };
        let best = |bytes: usize| read(bytes).min(read(bytes));
        let small = best(1 << 20);
        let large = best(4 << 20);
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        println!("1 MiB {small:?}, 4 MiB {large:?}: x{ratio:.1}");
        assert!(
            ratio < 8.0,
            "4x the bytes took x{ratio:.1} the time (linear ~4, quadratic ~16): \
             1 MiB {small:?}, 4 MiB {large:?}"
        );
    }

    /// Rows of `egret_expected.loop_value_to_string` egret knowingly does NOT
    /// reproduce, each with why: before the Gregorian cutover Kestra's
    /// `Calendar` reads the Julian calendar (`0000-12-30`), and an annotation
    /// that is not a `java.time` class makes Kestra's mapper fail where egret
    /// reads the value without it.
    const LOOP_VALUE_DIVERGENT: &[&str] = &["0001-01-01T00:00:00Z", r#"foo::"x""#];

    /// **A `Loop` over a file reads each value as Kestra's
    /// `FlowableUtils.ionValueToString` does** (the user's ruling of
    /// 2026-10-03 on W9a Task 11's concern 2): every row of
    /// `egret_expected.loop_value_to_string` (Kestra's own method, run on the
    /// artifact's jar in its container's zone) but the named divergent ones,
    /// which must still differ so the list cannot go stale. A `null` row is
    /// Kestra's message; a row `FileSerde.read` refused (`read_error`) is an
    /// `Err` here too.
    #[test]
    fn a_loop_reads_each_value_as_kestras_ion_value_to_string() {
        let o = oracle();
        let rows = o["egret_expected"]["loop_value_to_string"]
            .as_array()
            .unwrap();
        assert!(rows.len() >= 94, "{} rows", rows.len());
        let (mut matched, mut refused) = (0, 0);
        for row in rows {
            let ion = row["ion"].as_str().unwrap();
            let (values, stop) = loop_values(cursor(ion), 10, u64::MAX);
            assert!(matches!(stop, Stop::End), "{ion}: {stop:?}");
            assert_eq!(values.len(), 1, "{ion}");
            let ours = values.into_iter().next().unwrap();
            if LOOP_VALUE_DIVERGENT.contains(&ion) {
                assert!(
                    row["value"].as_str() != ours.as_deref().ok(),
                    "{ion} is listed as divergent but matches"
                );
            } else if row.get("read_error").is_some() {
                assert!(ours.is_err(), "{ion}: {ours:?}");
                refused += 1;
            } else if let Some(error) = row["error"].as_str() {
                assert_eq!(error, LOOP_NULL_RECORD, "{ion}");
                assert_eq!(ours, Err(LOOP_NULL_RECORD.to_string()), "{ion}");
                refused += 1;
            } else {
                assert_eq!(ours.as_deref(), Ok(row["value"].as_str().unwrap()), "{ion}");
                matched += 1;
            }
        }
        assert_eq!(matched + refused, rows.len() - LOOP_VALUE_DIVERGENT.len());
        assert_eq!(
            refused, 2,
            "the null.int row and the unparseable LocalDateTime"
        );
    }

    /// A control character in a string the `Loop`'s JSON writes is escaped
    /// as Jackson escapes it -- the four hex digits UPPER case (`\u001F`),
    /// which `serde_json` writes lower case (fix round 1; the rule
    /// `runners::env::write_json_string` already holds for `toJson`) -- in a
    /// value, in a field name and in a list.
    #[test]
    fn a_loops_json_escapes_control_characters_as_jackson_does() {
        let (values, _) = loop_values(cursor(r#"{'k\x0b':"a\x1fb"} ["c\x0ed"]"#), 10, u64::MAX);
        assert_eq!(
            values,
            vec![
                Ok(r#"{"k\u000B":"a\u001Fb"}"#.to_string()),
                Ok(r#"["c\u000Ed"]"#.to_string()),
            ]
        );
    }

    /// The preview's projection is NOT the `Loop`'s: Kestra's preview shows a
    /// decimal as a double and a timestamp as its Ion text (the table at the
    /// top of this module, measured), and those rows stay green above.
    #[test]
    fn the_preview_keeps_its_own_decimal_and_timestamp() {
        let (rows, _) = values_to_json(cursor("12.50 2025-12-03T12:00:00Z"), 10).unwrap();
        assert_eq!(rows, vec![json!(12.5), json!("2025-12-03T12:00:00Z")]);
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
