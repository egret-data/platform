//! `io.kestra.plugin.core.storage.{Concat, Split, Reverse, DeduplicateItems,
//! FilterItems}` -- the whole-file core storage tasks (W9a Task 12, spec
//! §9.3), each following its Kestra class (`Concat.java`, `Split.java` and
//! `StorageService.split`, `Reverse.java`, `DeduplicateItems.java`,
//! `FilterItems.java`), measured in `w9a-storage.json` `core_storage`.
//!
//! # What each does (Kestra's, measured where the fixture has a run)
//!
//! - **`Concat`** copies `files` in order, each followed by `separator`
//!   when one is given -- AFTER every file, the last included (measured,
//!   `concat_sep`). `files` is `Object` in Kestra: a list whose items are
//!   rendered one by one, or a string rendered, read as a JSON list, and
//!   each of its items rendered again (`Concat.java:161`).
//! - **`Split`** reads `from` as text ROWS (`BufferedReader.readLine`: a row
//!   ends at `\n`, `\r` or `\r\n`) -- or, when the file starts with binary
//!   Ion's version marker, as Ion values -- and writes each row followed by
//!   `separator` (Ion values as Ion text, one per line). `regexPattern`,
//!   `bytes`, `partitions`, `rows` are tried in that order: a new file once
//!   the current one holds `bytes` bytes or `rows` rows (`rows: 2` on five
//!   rows is `[2,2,1]`, `bytes: '6'` the same, measured), `partitions` files
//!   round-robin with the empty ones left out (`partitions: 2` is
//!   `[l1 l3 l5][l2 l4]`, measured), or one file per first capture group of
//!   `regexPattern`, in order of first appearance, a row that does not match
//!   dropped. The files take `from`'s extension (`.tmp` when it has none).
//! - **`Reverse`** writes `from`'s rows last first, each followed by
//!   `separator` -- a trailing terminator does not make an empty last row
//!   (`ReversedLinesFileReader`; `l1…l5\n` is `l5…l1\n`, measured).
//! - **`DeduplicateItems`** keeps, of the items with the same `expr` key,
//!   the LAST, in the order of those last items (`b`, `a` (v=3), `c`,
//!   `numKeys` 3, measured).
//! - **`FilterItems`** keeps (`INCLUDE`) or drops (`EXCLUDE`) the items whose
//!   `filterCondition` is truthy -- anything but `false`, `0`, `-0` or the
//!   empty string, trimmed (`TruthUtils`). An item whose expression FAILS
//!   follows `errorOrNullBehavior`: `FAIL` fails the task with the render's
//!   error, `INCLUDE` keeps it, `EXCLUDE` drops it -- never a worker error.
//!
//! # The expression of an item (`expr`, `filterCondition`)
//!
//! Rendered once per item, the item's fields bound as top-level names OVER
//! the run's own variables (Kestra's `runContext.render(expression, map)`,
//! `storage::ion::record_binding`), on egret's shared environment -- where
//! `secret()` is refused -- with the run's own file scope, so `read()` reads
//! only what the run's snapshot holds, and an outage met by one is a worker
//! failure (spec §11). **One pass** (`render_once_with`): an item's value
//! that holds `{{ … }}` is data, not a template -- Kestra's
//! `recursiveRendering` default, and no way for a file's contents to reach
//! a function the author did not write. An item that is not a struct fails
//! the task (Kestra's `convertValue(…, Map.class)` throws).
//!
//! # Where egret differs, on purpose
//!
//! - **Names are fixed per run** (spec §4.3; Kestra's are random temp-file
//!   numbers, measured): `Concat`, `Reverse`, `DeduplicateItems` and
//!   `FilterItems` write `content{ext}` ([`super::WRITE_FILE_STEM`]),
//!   `Split` `part-00000{ext}`, `part-00001{ext}`, … ([`SPLIT_FILE_STEM`]).
//! - **`DeduplicateItems` and `FilterItems` write Ion TEXT**, one item per
//!   line (`runners::ionfmt`); Kestra writes BINARY Ion (`FileSerde`,
//!   finding `store_binary_ion`), so the bytes differ and the values do not.
//!   `Split` of a binary Ion file writes Ion text too, and its `bytes` counts
//!   the text it wrote where Kestra counts each value's standalone binary
//!   size.
//! - **Nothing holds a whole file** (global constraints): every task streams
//!   its input and its outputs (`stream`), except `Reverse`, which reads its
//!   input whole and so only up to `workflow.storage.max_read_bytes` (Kestra
//!   copies it to disk). One Ion item is held whole, so an item longer than
//!   `max_read_bytes` is refused (`ion::for_each_value`). `DeduplicateItems`
//!   reads its input TWICE -- the keys, then the items it keeps -- and holds
//!   only each distinct key and its last item's index, each charged its
//!   bytes and [`KEY_OVERHEAD`] against `max_read_bytes`, where Kestra holds
//!   every item. `Split` by `partitions` or `regexPattern` writes at most
//!   [`KEYED_UPLOADS_PER_PASS`] files at a time and reads its input once per
//!   that many files, at most `max_task_bytes` in all, and writes at most
//!   [`MAX_SPLIT_FILES`] files in all.
//! - **`Reverse` reads only UTF-8** (`charset` another name is refused);
//!   Kestra decodes any Java charset.

use std::collections::{HashMap, HashSet};
use std::io::{BufReader, Read};
use std::ops::ControlFlow;
use std::sync::Arc;

use egret_core::{Error, Result};
use serde::Deserialize;

use super::super::super::{Outcome, TaskHandler, TaskInput};
use super::stream::{run_blocking, Faults, Halt, Input, Lines, Run, Step, Upload};
use super::{
    no_storage, outage, spec, CONCAT, DEDUPLICATE_ITEMS, DEFAULT_EXTENSION, FILTER_ITEMS, REVERSE,
    SPLIT, WRITE_FILE_STEM,
};
use crate::models::Property;
use crate::runners::files::FileScope;
use crate::runners::{render_str, RenderContext};
use crate::storage::ion::{self, BINARY_VERSION_MARKER};
use crate::storage::{FileStore, StoredFile};

/// The stem of `Split`'s files: `part-00000{ext}`, `part-00001{ext}`, …
pub const SPLIT_FILE_STEM: &str = "part-";

/// The most files one `Split` writes; one more is refused naming this
/// limit. Kestra has none: egret indexes every file and keeps every one
/// [`retention`](crate::storage) days, so a `rows: 1` over a large file is
/// refused rather than written as millions of objects.
pub const MAX_SPLIT_FILES: usize = 10_000;

/// How many files a `Split` by `partitions` or `regexPattern` writes at once:
/// each pass over the input writes this many and skips the rest, so the
/// memory is this many uploads' and not one per file.
pub const KEYED_UPLOADS_PER_PASS: usize = 8;

/// A failed task naming its class, or `Err` (spec §11).
fn settle(type_id: &str, step: Step<Outcome>) -> Result<Outcome> {
    match step {
        Ok(outcome) => Ok(outcome),
        Err(Halt::Author(why)) => Ok(Outcome::failed_with(format!("{type_id}: {why}"))),
        Err(Halt::Worker(why)) => Err(Error::Internal(anyhow::anyhow!("{type_id}: {why}"))),
        Err(Halt::Cancelled) => Err(Error::Internal(anyhow::anyhow!(
            "{type_id}: the run was cancelled"
        ))),
    }
}

/// What every one of the five needs before it reads a byte: the store, the
/// run's file scope, and its prefix -- or the failed task saying which is
/// missing.
fn prepared(
    type_id: &str,
    task: &TaskInput,
    files: &Option<Arc<FileStore>>,
) -> std::result::Result<(Arc<FileStore>, FileScope, String), String> {
    let Some(fs) = files.clone() else {
        return Err(no_storage(type_id));
    };
    let Some(scope) = task.render.files().cloned() else {
        return Err(no_storage(type_id));
    };
    let prefix = super::super::task_storage_prefix(task, type_id)?;
    Ok((fs, scope, prefix))
}

/// The indexed size of `file`, through the run's snapshot -- the one
/// readability decision (`storage::resolve_readable_many`): a file that is
/// not readable here, or not there, is the author's.
fn size_of(scope: &FileScope, file: &str) -> std::result::Result<u64, String> {
    scope
        .0
        .lookup(file)
        .map(|meta| meta.size)
        .map_err(|why| why.message().to_string())
}

/// `extension` as a file name's end: one holding `/` is refused, as `Write`
/// refuses it (Java's `createTempFile` throws for it).
fn checked_extension(extension: String) -> std::result::Result<String, String> {
    if extension.contains('/') {
        return Err(format!(
            "the extension is {extension:?}: it is the end of the file's name and may not hold \
             `/`"
        ));
    }
    Ok(extension)
}

/// `Split`'s extension: `StorageService.extensionOf` -- everything from the
/// LAST `.` of the URI's path, or `.tmp`.
fn split_extension(file: &str) -> std::result::Result<String, String> {
    let path = file.strip_prefix("kestra://").unwrap_or(file);
    match path.rfind('.') {
        Some(at) => checked_extension(path[at..].to_string()),
        None => Ok(DEFAULT_EXTENSION.to_string()),
    }
}

/// `Reverse`'s extension: `FilenameUtils.getExtension` of the file's NAME,
/// with its dot, or `.tmp` when it has none.
fn name_extension(file: &str) -> String {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    match name.rfind('.') {
        Some(at) if at + 1 < name.len() => name[at..].to_string(),
        _ => DEFAULT_EXTENSION.to_string(),
    }
}

/// Render `p`, or the failed task's message.
fn rendered<T: serde::de::DeserializeOwned>(
    p: &Property<T>,
    cx: &RenderContext,
) -> std::result::Result<T, String> {
    p.render(cx).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Concat
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ConcatSpec {
    files: serde_json::Value,
    #[serde(default)]
    separator: Option<Property<String>>,
    #[serde(default)]
    extension: Option<Property<String>>,
}

/// `core.storage.Concat`: `files` one after the other, as one file.
#[derive(Debug, Clone, Default)]
pub struct Concat {
    files: Option<Arc<FileStore>>,
}

impl Concat {
    pub fn new(files: Option<Arc<FileStore>>) -> Self {
        Self { files }
    }
}

/// `files` as Kestra reads it: a list whose items are rendered, or a string
/// rendered, read as a JSON list of strings, and each item rendered again.
fn concat_files(
    files: &serde_json::Value,
    cx: &RenderContext,
) -> std::result::Result<Vec<String>, String> {
    let items: Vec<String> = match files {
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| match item {
                serde_json::Value::String(s) => Ok(s.clone()),
                other => Err(format!(
                    "`files` holds {other}, which is not a string (a URI)"
                )),
            })
            .collect::<std::result::Result<_, _>>()?,
        serde_json::Value::String(s) => {
            let text = render_str(s, cx).map_err(|e| e.to_string())?;
            serde_json::from_str::<Vec<String>>(&text).map_err(|e| {
                format!("`files` rendered to {text:?}, which is not a JSON list of strings: {e}")
            })?
        }
        other => {
            return Err(format!(
                "Invalid `files` properties with type '{}': a list of URIs, or a string of one",
                match other {
                    serde_json::Value::Object(_) => "object",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::Bool(_) => "boolean",
                    _ => "null",
                }
            ))
        }
    };
    items
        .iter()
        .map(|item| render_str(item, cx).map_err(|e| e.to_string()))
        .collect()
}

#[async_trait::async_trait]
impl TaskHandler for Concat {
    fn type_id(&self) -> &'static str {
        CONCAT
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let fail = |why: String| Ok(Outcome::failed_with(format!("{CONCAT}: {why}")));
        let spec: ConcatSpec = match spec(CONCAT, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let (fs, scope, prefix) = match prepared(CONCAT, task, &self.files) {
            Ok(p) => p,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let files = concat_files(&spec.files, &task.render);
        let separator = spec
            .separator
            .as_ref()
            .map(|p| rendered(p, &task.render))
            .transpose();
        let extension = spec
            .extension
            .as_ref()
            .map(|p| rendered(p, &task.render))
            .transpose();
        outage(CONCAT, task)?;
        let (files, separator, extension) = match (files, separator, extension) {
            (Ok(f), Ok(s), Ok(e)) => (f, s, e.unwrap_or_else(|| DEFAULT_EXTENSION.to_string())),
            (Err(why), _, _) | (_, Err(why), _) | (_, _, Err(why)) => return fail(why),
        };
        let extension = match checked_extension(extension) {
            Ok(e) => e,
            Err(why) => return fail(why),
        };
        let mut inputs = Vec::new();
        for file in files {
            match size_of(&scope, &file) {
                Ok(size) => inputs.push((file, size)),
                Err(why) => return fail(why),
            }
        }
        // The caps from the index, before a byte is uploaded.
        let separator = separator.map(String::into_bytes).unwrap_or_default();
        let total: u64 = inputs
            .iter()
            .map(|(_, size)| size + separator.len() as u64)
            .sum();
        let caps = fs.caps;
        let name = format!("{WRITE_FILE_STEM}{extension}");
        if total > caps.max_file_bytes.min(caps.max_task_bytes) {
            let (cap, what) = if total > caps.max_file_bytes {
                (
                    caps.max_file_bytes,
                    "one file may be (workflow.storage.max_file_bytes)",
                )
            } else {
                (
                    caps.max_task_bytes,
                    "one task run may store (workflow.storage.max_task_bytes)",
                )
            };
            return fail(format!(
                "{name} would be {total} bytes, over the {cap} {what}"
            ));
        }
        let run = Run::new(fs, task.workspace_id, prefix);
        let step = run_blocking(run, move |run| {
            let mut out = run.create(&name)?;
            let mut buf = vec![0u8; super::stream::CHUNK];
            for (file, size) in &inputs {
                let (mut input, faults) = run.open(file, *size)?;
                loop {
                    let n = match input.read(&mut buf) {
                        Ok(n) => n,
                        Err(e) => return Err(faults.or_worker(file, &e)),
                    };
                    if n == 0 {
                        break;
                    }
                    out.write(&buf[..n])?;
                }
                out.write(&separator)?;
            }
            let file = out.finish()?;
            Ok(single(file))
        })
        .await;
        settle(CONCAT, step)
    }

    /// Holds nothing past run: dropping the run's future cancels its thread
    /// and aborts its upload (`stream::run_blocking`).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

/// A run's one file as `{uri}` and the file the outcome commit indexes.
fn single(file: StoredFile) -> Outcome {
    Outcome {
        outputs: Some(serde_json::json!({ "uri": file.uri })),
        files: vec![file],
        ..Outcome::succeeded_with(serde_json::Value::Null)
    }
}

// ---------------------------------------------------------------------------
// Reverse
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ReverseSpec {
    from: Property<String>,
    #[serde(default)]
    separator: Option<Property<String>>,
    #[serde(default)]
    charset: Option<Property<String>>,
}

/// `core.storage.Reverse`: `from`'s rows, last first.
#[derive(Debug, Clone, Default)]
pub struct Reverse {
    files: Option<Arc<FileStore>>,
}

impl Reverse {
    pub fn new(files: Option<Arc<FileStore>>) -> Self {
        Self { files }
    }
}

/// `bytes`'s rows, last first, as `ReversedLinesFileReader` yields them: a
/// row ends at `\r\n`, `\n` or `\r`, and the file's own trailing terminator
/// does not make an empty last row. An empty file has none.
pub(super) fn reversed_rows(bytes: &[u8]) -> Vec<&[u8]> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                rows.push(&bytes[start..i]);
                start = i + 1;
            }
            b'\r' => {
                rows.push(&bytes[start..i]);
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    // What follows the last terminator: a row, unless it is the empty one a
    // trailing terminator leaves.
    if start < bytes.len() {
        rows.push(&bytes[start..]);
    }
    rows.reverse();
    rows
}

#[async_trait::async_trait]
impl TaskHandler for Reverse {
    fn type_id(&self) -> &'static str {
        REVERSE
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let fail = |why: String| Ok(Outcome::failed_with(format!("{REVERSE}: {why}")));
        let spec: ReverseSpec = match spec(REVERSE, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let (fs, scope, prefix) = match prepared(REVERSE, task, &self.files) {
            Ok(p) => p,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let from = rendered(&spec.from, &task.render);
        let separator = spec
            .separator
            .as_ref()
            .map(|p| rendered(p, &task.render))
            .transpose();
        let charset = spec
            .charset
            .as_ref()
            .map(|p| rendered(p, &task.render))
            .transpose();
        outage(REVERSE, task)?;
        let (from, separator, charset) = match (from, separator, charset) {
            (Ok(f), Ok(s), Ok(c)) => (
                f,
                s.unwrap_or_else(|| "\n".to_string()),
                c.unwrap_or_else(|| "UTF-8".to_string()),
            ),
            (Err(why), _, _) | (_, Err(why), _) | (_, _, Err(why)) => return fail(why),
        };
        if !matches!(charset.to_ascii_uppercase().as_str(), "UTF-8" | "UTF8") {
            return fail(format!(
                "`charset` is {charset:?}: egret reverses UTF-8 files only"
            ));
        }
        let size = match size_of(&scope, &from) {
            Ok(size) => size,
            Err(why) => return fail(why),
        };
        let name = format!("{WRITE_FILE_STEM}{}", name_extension(&from));
        let run = Run::new(fs, task.workspace_id, prefix);
        let step = run_blocking(run, move |run| {
            let bytes = run.read_capped(
                &from,
                size,
                "Reverse reads the whole file into memory to reverse it",
            )?;
            let mut out = run.create(&name)?;
            for row in reversed_rows(&bytes) {
                // Decoded and encoded again, as Java does: a byte that is not
                // UTF-8 becomes U+FFFD.
                out.write(String::from_utf8_lossy(row).as_bytes())?;
                out.write(separator.as_bytes())?;
            }
            Ok(single(out.finish()?))
        })
        .await;
        settle(REVERSE, step)
    }

    /// Holds nothing past run (`stream::run_blocking`).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Split
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SplitSpec {
    from: Property<String>,
    #[serde(default)]
    bytes: Option<Property<String>>,
    #[serde(default)]
    partitions: Option<Property<i64>>,
    #[serde(default)]
    rows: Option<Property<i64>>,
    #[serde(rename = "regexPattern", default)]
    regex_pattern: Option<Property<String>>,
    #[serde(default)]
    separator: Option<Property<String>>,
}

/// How a `Split` cuts its input, in Kestra's order of precedence.
enum Cut {
    /// One file per first capture group, in order of first appearance.
    Regex(crate::runners::JavaRegex),
    /// A new file once the current one holds this many bytes.
    Bytes(i64),
    /// This many files, round-robin.
    Partitions(usize),
    /// A new file once the current one holds this many rows.
    Rows(i64),
}

/// `ReadableBytesTypeConverter`: a whole number of bytes, or one with a
/// `KB`, `MB` or `GB` suffix (any case, powers of 1024).
fn parse_bytes(text: &str) -> Option<i64> {
    let lower = text.trim().to_ascii_lowercase();
    let (digits, unit) = match () {
        _ if lower.ends_with("kb") => (&lower[..lower.len() - 2], 1i64 << 10),
        _ if lower.ends_with("mb") => (&lower[..lower.len() - 2], 1 << 20),
        _ if lower.ends_with("gb") => (&lower[..lower.len() - 2], 1 << 30),
        _ => (lower.as_str(), 1),
    };
    digits.parse::<i64>().ok()?.checked_mul(unit)
}

/// `core.storage.Split`: `from` cut into files.
#[derive(Debug, Clone, Default)]
pub struct Split {
    files: Option<Arc<FileStore>>,
}

impl Split {
    pub fn new(files: Option<Arc<FileStore>>) -> Self {
        Self { files }
    }
}

/// One record of a `Split`'s input: the bytes it is written as, and the
/// text a `regexPattern` is matched against (Kestra's `routingText`).
struct Record<'a> {
    bytes: &'a [u8],
    text: &'a str,
}

/// Every record of `file`, in order, to `each` -- text rows, or Ion values
/// when the file starts with binary Ion's version marker (Kestra's
/// `IonStreamUtils.isIonBinary` on its first four bytes). Streamed: one
/// record is held at a time. `each` answers `Ok(false)` to stop early.
fn each_record(
    run: &Run,
    file: &str,
    size: u64,
    separator: &str,
    each: &mut (dyn FnMut(Record<'_>) -> Step<bool> + Send),
) -> Step<()> {
    let (mut input, faults) = run.open(file, size)?;
    let mut head = Vec::with_capacity(4);
    if let Err(e) = (&mut input).take(4).read_to_end(&mut head) {
        return Err(faults.or_worker(file, &e));
    }
    let binary = head == BINARY_VERSION_MARKER;
    let whole = std::io::Cursor::new(head).chain(input);
    if binary {
        return each_ion_value(run, file, whole, &faults, each);
    }
    let mut lines = Lines::new(
        BufReader::with_capacity(super::stream::CHUNK, whole),
        file,
        faults,
        run.fs.caps.max_read_bytes,
    );
    let mut bytes = Vec::new();
    while let Some(row) = lines.next_row()? {
        run.live()?;
        // Decoded as Java's reader decodes it: a byte that is not UTF-8 is
        // U+FFFD.
        let text = String::from_utf8_lossy(&row);
        bytes.clear();
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(separator.as_bytes());
        if !each(Record {
            bytes: &bytes,
            text: &text,
        })? {
            break;
        }
    }
    Ok(())
}

/// [`each_record`] over Ion values: each written as Ion text and a newline.
fn each_ion_value(
    run: &Run,
    file: &str,
    input: impl Read + Send,
    faults: &Faults,
    each: &mut (dyn FnMut(Record<'_>) -> Step<bool> + Send),
) -> Step<()> {
    let mut halted = None;
    let mut text = String::new();
    let stop = ion::for_each_value(input, run.fs.caps.max_read_bytes, |element| {
        let step = run.live().and_then(|()| {
            text.clear();
            crate::runners::write_ion_element(&mut text, element);
            let routing = text.clone();
            text.push('\n');
            each(Record {
                bytes: text.as_bytes(),
                text: &routing,
            })
        });
        match step {
            Ok(true) => ControlFlow::Continue(()),
            Ok(false) => ControlFlow::Break(()),
            Err(halt) => {
                halted = Some(halt);
                ControlFlow::Break(())
            }
        }
    });
    if let Some(halt) = halted {
        return Err(halt);
    }
    faults.after_ion(file, stop)
}

/// The name of `Split`'s `n`th file.
fn part_name(n: usize, extension: &str) -> String {
    format!("{SPLIT_FILE_STEM}{n:05}{extension}")
}

fn too_many_files(max_files: usize) -> Halt {
    Halt::Author(format!(
        "it would write more than {max_files} files, egret's limit for one Split"
    ))
}

/// `bytes` and `rows`: one file at a time, a new one once the current one
/// holds `limit` of `measure` (Kestra's `splitByPredicate`: checked before
/// each record, so a file may end over `bytes`).
fn split_sequential(
    run: &Run,
    file: &str,
    size: u64,
    separator: &str,
    extension: &str,
    limit: i64,
    by_bytes: bool,
) -> Step<Vec<StoredFile>> {
    let mut done: Vec<StoredFile> = Vec::new();
    let mut current: Option<(Upload<'_>, i64)> = None;
    each_record(run, file, size, separator, &mut |record| {
        let full = match &current {
            None => true,
            Some((_, held)) => *held >= limit,
        };
        if full {
            if let Some((upload, _)) = current.take() {
                done.push(upload.finish()?);
            }
            if done.len() == MAX_SPLIT_FILES {
                return Err(too_many_files(MAX_SPLIT_FILES));
            }
            current = Some((run.create(&part_name(done.len(), extension))?, 0));
        }
        let (upload, held) = current.as_mut().expect("opened above");
        upload.write(record.bytes)?;
        *held += if by_bytes {
            record.bytes.len() as i64
        } else {
            1
        };
        Ok(true)
    })?;
    if let Some((upload, _)) = current.take() {
        done.push(upload.finish()?);
    }
    Ok(done)
}

/// `partitions` and `regexPattern`: each record goes to a file named by its
/// key, files written [`KEYED_UPLOADS_PER_PASS`] at a time -- each pass over
/// the input writes the next keys and skips the rest. A key no record
/// reaches has no file (Kestra drops its empty partitions). Files are listed
/// in key order: the partition's index, or the order of first appearance.
///
/// **The passes are bounded** (W9a Task 12 fix round 1): before each pass,
/// the bytes read so far and this pass's must fit in `max_task_bytes`
/// ([`over_read_budget`], the same check `partitions` meets up front), and a
/// pass is read only while one is needed -- a partition no record reaches
/// (`partitions` over fewer records) or a key not yet met (`regexPattern`)
/// -- so eight keys cost one pass, not two. The keys already written are
/// held as [`KeyDigest`]s, not as the keys: a key can be a row long.
fn split_keyed(
    run: &Run,
    file: &str,
    size: u64,
    separator: &str,
    extension: &str,
    cut: &Cut,
    max_files: usize,
) -> Step<Vec<StoredFile>> {
    let mut done: Vec<StoredFile> = Vec::new();
    let mut seen: HashSet<KeyDigest> = HashSet::new();
    let mut pass = 0usize;
    loop {
        if let Some(why) = over_read_budget(pass + 1, file, size, run.fs.caps.max_task_bytes) {
            return Err(Halt::Author(why));
        }
        let mut active: Vec<(Option<String>, Option<Upload<'_>>)> = Vec::new();
        let mut index = 0usize;
        let mut unmet = false;
        let first = done.len();
        each_record(run, file, size, separator, &mut |record| {
            let key = match cut {
                Cut::Partitions(n) => {
                    let key = index % n;
                    index += 1;
                    // This pass's partitions only.
                    if key / KEYED_UPLOADS_PER_PASS != pass {
                        return Ok(true);
                    }
                    Some(key.to_string())
                }
                Cut::Regex(regex) => match regex.find_group(record.text, 1) {
                    Ok(Some(group)) if regex.group_count() > 0 => group,
                    Ok(_) => return Ok(true),
                    Err(e) => return Err(Halt::Author(format!("`regexPattern`: {e}"))),
                },
                Cut::Bytes(_) | Cut::Rows(_) => unreachable!("sequential cuts"),
            };
            let slot = match active.iter().position(|(k, _)| *k == key) {
                Some(slot) => slot,
                None => {
                    let digest = KeyDigest::of(key.as_deref());
                    if seen.contains(&digest) {
                        return Ok(true);
                    }
                    if active.len() == KEYED_UPLOADS_PER_PASS {
                        // A key for a later pass.
                        unmet = true;
                        return Ok(true);
                    }
                    if first + active.len() == max_files {
                        return Err(too_many_files(max_files));
                    }
                    seen.insert(digest);
                    active.push((key, None));
                    active.len() - 1
                }
            };
            if active[slot].1.is_none() {
                let n = match cut {
                    Cut::Partitions(_) => active[slot]
                        .0
                        .as_deref()
                        .and_then(|k| k.parse().ok())
                        .unwrap_or(0),
                    _ => first + slot,
                };
                active[slot].1 = Some(run.create(&part_name(n, extension))?);
            }
            active[slot]
                .1
                .as_mut()
                .expect("created above")
                .write(record.bytes)?;
            Ok(true)
        })?;
        for (_, upload) in active {
            if let Some(upload) = upload {
                done.push(upload.finish()?);
            }
        }
        pass += 1;
        let next = pass * KEYED_UPLOADS_PER_PASS;
        let more = match cut {
            // A later pass's partitions exist, and a record reaches one.
            Cut::Partitions(n) => next < *n && next < index,
            _ => unmet,
        };
        if !more {
            return Ok(done);
        }
    }
}

/// A key a keyed `Split` has written, held as the first 128 bits of its
/// SHA-256 (a `None` key -- a group that did not take part -- apart from
/// every string): 16 bytes whatever the key's length.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct KeyDigest([u8; 16]);

impl KeyDigest {
    fn of(key: Option<&str>) -> Self {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        match key {
            None => hasher.update([0u8]),
            Some(key) => {
                hasher.update([1u8]);
                hasher.update(key.as_bytes());
            }
        }
        let mut out = [0u8; 16];
        out.copy_from_slice(&hasher.finalize()[..16]);
        Self(out)
    }
}

/// Why `passes` reads of `file` (`size` bytes each) are refused: more than
/// `budget` bytes in all -- `workflow.storage.max_task_bytes`, the cap on
/// what one task run stores, which a keyed `Split` meets for what it reads
/// too. `None` when they fit. The one check, up front for `partitions` and
/// before every pass.
fn over_read_budget(passes: usize, file: &str, size: u64, budget: u64) -> Option<String> {
    let read = (passes as u64).saturating_mul(size);
    (read > budget).then(|| {
        format!(
            "it would read {file} {passes} passes of {size} bytes ({KEYED_UPLOADS_PER_PASS} \
             files a pass), {read} bytes, over the {budget} bytes one task run may read and \
             store (workflow.storage.max_task_bytes)"
        )
    })
}

#[async_trait::async_trait]
impl TaskHandler for Split {
    fn type_id(&self) -> &'static str {
        SPLIT
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let fail = |why: String| Ok(Outcome::failed_with(format!("{SPLIT}: {why}")));
        let spec: SplitSpec = match spec(SPLIT, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let (fs, scope, prefix) = match prepared(SPLIT, task, &self.files) {
            Ok(p) => p,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let cx = &task.render;
        let from = rendered(&spec.from, cx);
        let separator = spec.separator.as_ref().map(|p| rendered(p, cx)).transpose();
        // `StorageService.split`'s order: regexPattern, bytes, partitions, rows.
        let cut = if let Some(p) = &spec.regex_pattern {
            rendered(p, cx).and_then(|pattern| {
                crate::runners::compile_java_regex(&pattern)
                    .map(Cut::Regex)
                    .map_err(|e| format!("`regexPattern` {pattern:?}: {e}"))
            })
        } else if let Some(p) = &spec.bytes {
            rendered(p, cx).and_then(|text| {
                parse_bytes(&text)
                    .map(Cut::Bytes)
                    .ok_or_else(|| format!("Invalid size with value '{text}'"))
            })
        } else if let Some(p) = &spec.partitions {
            rendered(p, cx).and_then(|n| match usize::try_from(n) {
                Ok(n) if (1..=MAX_SPLIT_FILES).contains(&n) => Ok(Cut::Partitions(n)),
                Ok(n) if n > MAX_SPLIT_FILES => Err(too_many_files_message(n)),
                _ => Err(format!("`partitions` is {n}: it must be at least 1")),
            })
        } else if let Some(p) = &spec.rows {
            rendered(p, cx).map(Cut::Rows)
        } else {
            Err("Invalid configuration with no size, count, rows, nor regexPattern".to_string())
        };
        outage(SPLIT, task)?;
        let (from, separator, cut) = match (from, separator, cut) {
            (Ok(f), Ok(s), Ok(c)) => (f, s.unwrap_or_else(|| "\n".to_string()), c),
            (Err(why), _, _) | (_, Err(why), _) | (_, _, Err(why)) => return fail(why),
        };
        let extension = match split_extension(&from) {
            Ok(e) => e,
            Err(why) => return fail(why),
        };
        let size = match size_of(&scope, &from) {
            Ok(size) => size,
            Err(why) => return fail(why),
        };
        if let Cut::Partitions(n) = &cut {
            let passes = n.div_ceil(KEYED_UPLOADS_PER_PASS);
            if let Some(why) = over_read_budget(passes, &from, size, fs.caps.max_task_bytes) {
                return fail(why);
            }
        }
        let run = Run::new(fs, task.workspace_id, prefix);
        let step = run_blocking(run, move |run| {
            let files = match &cut {
                Cut::Bytes(limit) => {
                    split_sequential(run, &from, size, &separator, &extension, *limit, true)?
                }
                Cut::Rows(limit) => {
                    split_sequential(run, &from, size, &separator, &extension, *limit, false)?
                }
                Cut::Partitions(_) | Cut::Regex(_) => split_keyed(
                    run,
                    &from,
                    size,
                    &separator,
                    &extension,
                    &cut,
                    MAX_SPLIT_FILES,
                )?,
            };
            let uris: Vec<&str> = files.iter().map(|f| f.uri.as_str()).collect();
            Ok(Outcome {
                outputs: Some(serde_json::json!({ "uris": uris })),
                files,
                ..Outcome::succeeded_with(serde_json::Value::Null)
            })
        })
        .await;
        settle(SPLIT, step)
    }

    /// Holds nothing past run (`stream::run_blocking`).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

fn too_many_files_message(n: usize) -> String {
    format!("`partitions` is {n}: egret writes at most {MAX_SPLIT_FILES} files for one Split")
}

// ---------------------------------------------------------------------------
// The item tasks: DeduplicateItems, FilterItems
// ---------------------------------------------------------------------------

/// `expression` rendered over `item` (its fields bound over `base`, see the
/// module doc), or why not: the render's error, or an outage a file function
/// met inside it (`Halt::Worker`).
fn render_item(
    expression: &str,
    base: &RenderContext,
    item: &ion_rs::Element,
) -> Step<std::result::Result<String, String>> {
    let fields = match ion::record_binding(item) {
        Ok(fields) => fields,
        Err(why) => return Err(Halt::Author(why)),
    };
    let mut cx = base.clone();
    for (name, value) in fields {
        // No template can name a field with a space, and the render's own
        // globals (`egret file scope`, `egret context scope`) have such
        // names: bound, the field would hide them.
        if name.contains(' ') {
            continue;
        }
        cx.insert(name, value);
    }
    let rendered = crate::runners::env::render_once_with(expression, &cx, &|_| {});
    if let Some(why) = base.files().and_then(|f| f.0.infrastructure_failure()) {
        return Err(Halt::Worker(why));
    }
    Ok(rendered.map_err(|e| e.to_string()))
}

/// The item's Ion text, for a message.
fn item_text(item: &ion_rs::Element) -> String {
    let mut text = String::new();
    crate::runners::write_ion_element(&mut text, item);
    text
}

/// Every item of `file` to `each` with its index, streamed; the first halt
/// stops the read and is the answer.
fn each_item(
    run: &Run,
    file: &str,
    size: u64,
    each: &mut (dyn FnMut(usize, &ion_rs::Element) -> Step<()> + Send),
) -> Step<()> {
    let (input, faults) = run.open(file, size)?;
    each_item_of(run, file, input, &faults, each)
}

fn each_item_of(
    run: &Run,
    file: &str,
    input: Input,
    faults: &Faults,
    each: &mut (dyn FnMut(usize, &ion_rs::Element) -> Step<()> + Send),
) -> Step<()> {
    let mut halted = None;
    let mut index = 0usize;
    let stop = ion::for_each_value(input, run.fs.caps.max_read_bytes, |element| {
        let step = run.live().and_then(|()| each(index, element));
        index += 1;
        match step {
            Ok(()) => ControlFlow::Continue(()),
            Err(halt) => {
                halted = Some(halt);
                ControlFlow::Break(())
            }
        }
    });
    if let Some(halt) = halted {
        return Err(halt);
    }
    faults.after_ion(file, stop)
}

/// The item tasks' file: `content.ion`.
fn items_name() -> String {
    format!("{WRITE_FILE_STEM}.ion")
}

/// One kept item, as Ion text and a newline.
fn write_item(out: &mut Upload<'_>, item: &ion_rs::Element) -> Step<()> {
    let mut text = item_text(item);
    text.push('\n');
    out.write(text.as_bytes())
}

#[derive(Debug, Deserialize)]
struct DeduplicateSpec {
    from: Property<String>,
    /// `@PluginProperty` with `dynamic = false`: the template itself, rendered
    /// per item, never as a property.
    expr: String,
}

/// `core.storage.DeduplicateItems`: of the items sharing an `expr` key, the
/// last.
#[derive(Debug, Clone, Default)]
pub struct DeduplicateItems {
    files: Option<Arc<FileStore>>,
}

impl DeduplicateItems {
    pub fn new(files: Option<Arc<FileStore>>) -> Self {
        Self { files }
    }
}

/// What [`DeduplicateItems`] charges one distinct key against
/// `workflow.storage.max_read_bytes` beyond the key's own bytes: its
/// `String`, its last item's index, the hash map's entry and control byte at
/// its load factor, and its place in the kept list -- about 50 bytes on a
/// 64-bit target, rounded up. An item whose key was met before costs nothing.
pub(super) const KEY_OVERHEAD: u64 = 64;

#[async_trait::async_trait]
impl TaskHandler for DeduplicateItems {
    fn type_id(&self) -> &'static str {
        DEDUPLICATE_ITEMS
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let fail = |why: String| Ok(Outcome::failed_with(format!("{DEDUPLICATE_ITEMS}: {why}")));
        let spec: DeduplicateSpec = match spec(DEDUPLICATE_ITEMS, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let (fs, scope, prefix) = match prepared(DEDUPLICATE_ITEMS, task, &self.files) {
            Ok(p) => p,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let from = rendered(&spec.from, &task.render);
        outage(DEDUPLICATE_ITEMS, task)?;
        let from = match from {
            Ok(f) => f,
            Err(why) => return fail(why),
        };
        let size = match size_of(&scope, &from) {
            Ok(size) => size,
            Err(why) => return fail(why),
        };
        let base = task.render.clone();
        let expr = spec.expr;
        let run = Run::new(fs, task.workspace_id, prefix);
        let step = run_blocking(run, move |run| {
            // Pass 1: each distinct key and its LAST item's index
            // (`DeduplicateItems.java`'s index map, measured: `b`, `a` (v=3),
            // `c`); what is held is charged, key by key, against the cap.
            let cap = run.fs.caps.max_read_bytes;
            let mut last: HashMap<String, u64> = HashMap::new();
            let mut held = 0u64;
            let mut items = 0u64;
            each_item(run, &from, size, &mut |i, item| {
                let key = render_item(&expr, &base, item)
                    .map_err(|halt| match halt {
                        Halt::Author(why) => {
                            Halt::Author(format!("item {i} of {from} is not a record: {why}"))
                        }
                        other => other,
                    })?
                    .map_err(|e| {
                        Halt::Author(format!(
                            "`expr` failed on item {i} of {from} ({}): {e}",
                            item_text(item)
                        ))
                    })?;
                items += 1;
                if let Some(at) = last.get_mut(&key) {
                    *at = i as u64;
                    return Ok(());
                }
                held += key.len() as u64 + KEY_OVERHEAD;
                if held > cap {
                    return Err(Halt::Author(format!(
                        "the keys of {from} are larger than workflow.storage.max_read_bytes \
                         ({cap} bytes): DeduplicateItems holds every distinct key (its bytes \
                         and {KEY_OVERHEAD} more), so it reads only files whose keys fit in \
                         that size"
                    )));
                }
                last.insert(key, i as u64);
                Ok(())
            })?;
            let num_keys = last.len();
            let mut keep: Vec<u64> = last.into_values().collect();
            keep.sort_unstable();
            // Pass 2: the kept items, as they stream past again.
            let changed = || {
                Halt::Worker(format!(
                    "{from} held a different number of items on its second read than the \
                     {items} of its first"
                ))
            };
            let mut out = run.create(&items_name())?;
            let mut processed = 0u64;
            let mut dropped = 0u64;
            let mut next = keep.iter().peekable();
            each_item(run, &from, size, &mut |i, item| {
                // An item past the first read's count matches no kept index
                // and is counted, so the check after the pass refuses more
                // items as it refuses fewer (one check: an in-pass one was
                // measured as dead weight, fix round 1's mutation I3e).
                let i = i as u64;
                if next.peek() == Some(&&i) {
                    next.next();
                    write_item(&mut out, item)?;
                } else {
                    dropped += 1;
                }
                processed += 1;
                Ok(())
            })?;
            if processed != items {
                return Err(changed());
            }
            let file = out.finish()?;
            Ok(Outcome {
                outputs: Some(serde_json::json!({
                    "uri": file.uri,
                    "numKeys": num_keys,
                    "processedItemsTotal": processed,
                    "droppedItemsTotal": dropped,
                })),
                files: vec![file],
                ..Outcome::succeeded_with(serde_json::Value::Null)
            })
        })
        .await;
        settle(DEDUPLICATE_ITEMS, step)
    }

    /// Holds nothing past run (`stream::run_blocking`).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct FilterSpec {
    from: Property<String>,
    /// `@PluginProperty` with `dynamic = false`, as `DeduplicateItems.expr`.
    #[serde(rename = "filterCondition")]
    filter_condition: String,
    #[serde(rename = "filterType", default)]
    filter_type: Option<Property<String>>,
    #[serde(rename = "errorOrNullBehavior", default)]
    error_or_null_behavior: Option<Property<String>>,
}

/// What `FilterItems` does with an item: Kestra's `FilterType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Keep {
    Include,
    Exclude,
}

impl Keep {
    fn parse(what: &str, text: &str) -> std::result::Result<Self, String> {
        match text {
            "INCLUDE" => Ok(Keep::Include),
            "EXCLUDE" => Ok(Keep::Exclude),
            other => Err(format!(
                "`{what}` is {other:?}: it is one of INCLUDE, EXCLUDE{}",
                if what == "errorOrNullBehavior" {
                    ", FAIL"
                } else {
                    ""
                }
            )),
        }
    }
}

/// `TruthUtils.isTruthy` of the rendered text, trimmed as Java's
/// `String.trim` trims: every char at or under U+0020, and nothing else
/// (not a no-break space, which Rust's `trim` would take).
pub(super) fn truthy(rendered: &str) -> bool {
    !matches!(
        rendered.trim_matches(|c: char| c <= '\u{20}'),
        "false" | "0" | "-0" | ""
    )
}

/// `FilterItems.run`'s decision for one item: `Ok(Some(action))` for the
/// item's action, given the condition's render and `on_error` (`None` is
/// `FAIL`); `Err` is the failure `FAIL` gives an item whose condition failed.
pub(super) fn decide(
    filter_type: Keep,
    on_error: Option<Keep>,
    condition: std::result::Result<String, String>,
) -> std::result::Result<Keep, String> {
    match condition {
        Ok(text) if truthy(&text) => Ok(filter_type),
        Ok(_) => Ok(match filter_type {
            Keep::Include => Keep::Exclude,
            Keep::Exclude => Keep::Include,
        }),
        // `match == null`: the behaviour IS the action, not reversed.
        Err(e) => on_error.ok_or(e),
    }
}

/// `core.storage.FilterItems`: the items `filterCondition` keeps.
#[derive(Debug, Clone, Default)]
pub struct FilterItems {
    files: Option<Arc<FileStore>>,
}

impl FilterItems {
    pub fn new(files: Option<Arc<FileStore>>) -> Self {
        Self { files }
    }
}

#[async_trait::async_trait]
impl TaskHandler for FilterItems {
    fn type_id(&self) -> &'static str {
        FILTER_ITEMS
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let fail = |why: String| Ok(Outcome::failed_with(format!("{FILTER_ITEMS}: {why}")));
        let spec: FilterSpec = match spec(FILTER_ITEMS, task) {
            Ok(s) => s,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let (fs, scope, prefix) = match prepared(FILTER_ITEMS, task, &self.files) {
            Ok(p) => p,
            Err(why) => return Ok(Outcome::failed_with(why)),
        };
        let cx = &task.render;
        let from = rendered(&spec.from, cx);
        let filter_type = spec
            .filter_type
            .as_ref()
            .map(|p| rendered(p, cx))
            .transpose()
            .and_then(|t| Keep::parse("filterType", t.as_deref().unwrap_or("INCLUDE")));
        let on_error = spec
            .error_or_null_behavior
            .as_ref()
            .map(|p| rendered(p, cx))
            .transpose()
            .and_then(|t| match t.as_deref().unwrap_or("FAIL") {
                "FAIL" => Ok(None),
                other => Keep::parse("errorOrNullBehavior", other).map(Some),
            });
        outage(FILTER_ITEMS, task)?;
        let (from, filter_type, on_error) = match (from, filter_type, on_error) {
            (Ok(f), Ok(t), Ok(e)) => (f, t, e),
            (Err(why), _, _) | (_, Err(why), _) | (_, _, Err(why)) => return fail(why),
        };
        let size = match size_of(&scope, &from) {
            Ok(size) => size,
            Err(why) => return fail(why),
        };
        let base = task.render.clone();
        let condition = spec.filter_condition;
        let run = Run::new(fs, task.workspace_id, prefix);
        let step = run_blocking(run, move |run| {
            let mut out = run.create(&items_name())?;
            let mut processed = 0u64;
            let mut dropped = 0u64;
            each_item(run, &from, size, &mut |i, item| {
                let rendered = render_item(&condition, &base, item).map_err(|halt| match halt {
                    Halt::Author(why) => {
                        Halt::Author(format!("item {i} of {from} is not a record: {why}"))
                    }
                    other => other,
                })?;
                let action = decide(filter_type, on_error, rendered).map_err(|e| {
                    Halt::Author(format!(
                        "`filterCondition` failed on item {i} of {from} ({}): {e}",
                        item_text(item)
                    ))
                })?;
                match action {
                    Keep::Include => write_item(&mut out, item)?,
                    Keep::Exclude => dropped += 1,
                }
                processed += 1;
                Ok(())
            })?;
            let file = out.finish()?;
            Ok(Outcome {
                outputs: Some(serde_json::json!({
                    "uri": file.uri,
                    "processedItemsTotal": processed,
                    "droppedItemsTotal": dropped,
                })),
                files: vec![file],
                ..Outcome::succeeded_with(serde_json::Value::Null)
            })
        })
        .await;
        settle(FILTER_ITEMS, step)
    }

    /// Holds nothing past run (`stream::run_blocking`).
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::testing::input;
    use super::*;
    use crate::executor::RunState;
    use crate::storage::scope::Snapshot;
    use crate::storage::{uri, Caps, IndexedFile, Unreadable};
    use egret_storage::{MemStore, ObjectStore, StorageError};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
    use uuid::Uuid;

    const EX: Uuid = Uuid::from_u128(0xe1);
    const RUN: Uuid = Uuid::from_u128(0xa1);
    const WRITER: Uuid = Uuid::from_u128(0xa2);

    fn store_with(store: Arc<dyn ObjectStore>, caps: Caps) -> Arc<FileStore> {
        Arc::new(FileStore {
            store,
            bucket: "egret-test".into(),
            prefix: "workflow/".into(),
            caps,
            retention_days: 30,
        })
    }

    fn mem() -> Arc<FileStore> {
        store_with(Arc::new(MemStore::new()), Caps::default())
    }

    /// The run under test's own prefix.
    fn prefix() -> String {
        uri::task_prefix("w9a.t12", "unit", EX, "t", RUN).unwrap()
    }

    /// Where the input files live: another task run of the same execution.
    fn input_uri(name: &str) -> String {
        format!(
            "{}{name}",
            uri::task_prefix("w9a.t12", "unit", EX, "w", WRITER).unwrap()
        )
    }

    type Files = BTreeMap<String, std::result::Result<IndexedFile, Unreadable>>;

    /// Store `bytes` as the input `name`, and index it in `files`.
    async fn stored(fs: &FileStore, files: &mut Files, name: &str, bytes: &[u8]) -> String {
        let file = input_uri(name);
        fs.store
            .put(&fs.object_uri(Uuid::nil(), &file).unwrap(), bytes.to_vec())
            .await
            .unwrap();
        files.insert(
            file.clone(),
            Ok(IndexedFile {
                uri: file.clone(),
                execution_id: EX,
                task_run_id: Some(WRITER),
                size: bytes.len() as u64,
            }),
        );
        file
    }

    fn task_over(
        type_id: &str,
        spec: serde_json::Value,
        fs: &Arc<FileStore>,
        files: Files,
    ) -> TaskInput {
        let mut t = input(type_id, spec);
        t.execution_id = EX;
        t.run_id = RUN;
        let snap = Snapshot::from_parts(files, fs.clone(), t.workspace_id, Some(prefix()));
        t.render = RenderContext::new().with_files(snap.into_scope());
        t
    }

    fn message(o: &Outcome) -> &str {
        assert_eq!(o.state, RunState::Failed, "{o:?}");
        o.message.as_deref().unwrap_or_default()
    }

    async fn object(fs: &FileStore, file: &str) -> Option<Vec<u8>> {
        match fs
            .store
            .get(&fs.object_uri(Uuid::nil(), file).unwrap())
            .await
        {
            Ok(bytes) => Some(bytes),
            Err(StorageError::NotFound(_)) => None,
            Err(e) => panic!("{file}: {e}"),
        }
    }

    /// The one output file's text.
    async fn text_of(fs: &FileStore, out: &Outcome) -> String {
        assert_eq!(out.state, RunState::Success, "{out:?}");
        let file = out.outputs.as_ref().unwrap()["uri"].as_str().unwrap();
        String::from_utf8(object(fs, file).await.unwrap()).unwrap()
    }

    /// The items of an Ion file, as JSON (the one Ion reader).
    fn items_of(bytes: &[u8]) -> Vec<serde_json::Value> {
        ion::values_to_json(std::io::Cursor::new(bytes.to_vec()), usize::MAX)
            .unwrap()
            .0
    }

    const ITEMS: &str = "{k:\"a\",v:1}\n{k:\"b\",v:2}\n{k:\"a\",v:3}\n{k:\"c\",v:4}";

    // ---- Split never holds its input ----------------------------------------

    /// `MemStore` behind a meter: `read` counts the bytes the task has read
    /// from an input, `consumed` the bytes the store has taken from an upload,
    /// and `peak` the most of the first not yet in the second at any moment
    /// -- what the task held. A `put_stream` takes its body 8 KiB at a time.
    struct Meter {
        inner: MemStore,
        read: Arc<AtomicU64>,
        consumed: Arc<AtomicU64>,
        peak: Arc<AtomicU64>,
    }

    impl Meter {
        fn new() -> Self {
            Self {
                inner: MemStore::new(),
                read: Arc::default(),
                consumed: Arc::default(),
                peak: Arc::default(),
            }
        }
        fn sample(read: &AtomicU64, consumed: &AtomicU64, peak: &AtomicU64) {
            let held = read
                .load(Ordering::SeqCst)
                .saturating_sub(consumed.load(Ordering::SeqCst));
            peak.fetch_max(held, Ordering::SeqCst);
        }
    }

    struct Counted {
        inner: Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
        read: Arc<AtomicU64>,
        consumed: Arc<AtomicU64>,
        peak: Arc<AtomicU64>,
    }

    impl AsyncRead for Counted {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            let before = buf.filled().len();
            let poll = std::pin::Pin::new(&mut self.inner).poll_read(cx, buf);
            let n = (buf.filled().len() - before) as u64;
            self.read.fetch_add(n, Ordering::SeqCst);
            Meter::sample(&self.read, &self.consumed, &self.peak);
            poll
        }
    }

    #[async_trait::async_trait]
    impl ObjectStore for Meter {
        async fn put(&self, uri: &str, bytes: Vec<u8>) -> std::result::Result<(), StorageError> {
            self.inner.put(uri, bytes).await
        }
        async fn get(&self, uri: &str) -> std::result::Result<Vec<u8>, StorageError> {
            self.inner.get(uri).await
        }
        async fn delete(&self, uri: &str) -> std::result::Result<(), StorageError> {
            self.inner.delete(uri).await
        }
        async fn copy(&self, a: &str, b: &str) -> std::result::Result<(), StorageError> {
            self.inner.copy(a, b).await
        }
        async fn list(
            &self,
            p: &str,
            d: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, StorageError> {
            self.inner.list(p, d).await
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> std::result::Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError>
        {
            let inner = self.inner.get_stream(uri).await?;
            Ok(Box::new(tokio::io::BufReader::with_capacity(
                8 << 10,
                Counted {
                    inner,
                    read: self.read.clone(),
                    consumed: self.consumed.clone(),
                    peak: self.peak.clone(),
                },
            )))
        }
        async fn put_stream(
            &self,
            uri: &str,
            body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            _: Option<u64>,
        ) -> std::result::Result<u64, StorageError> {
            let mut all = Vec::new();
            let mut buf = vec![0u8; 8 << 10];
            loop {
                let n = body
                    .read(&mut buf)
                    .await
                    .map_err(|e| StorageError::S3(format!("{uri}: {e}")))?;
                if n == 0 {
                    break;
                }
                self.consumed.fetch_add(n as u64, Ordering::SeqCst);
                Meter::sample(&self.read, &self.consumed, &self.peak);
                all.extend_from_slice(&buf[..n]);
            }
            let n = all.len() as u64;
            self.inner.put(uri, all).await?;
            Ok(n)
        }
        async fn head(&self, uri: &str) -> std::result::Result<Option<u64>, StorageError> {
            self.inner.head(uri).await
        }
    }

    /// **`Split` by `rows`, `bytes` and `partitions` never holds its input**:
    /// over a 20 MiB file, what it has read and the store has not yet taken
    /// stays under 1 MiB at every moment, and the files it wrote are the
    /// input, cut (spec global constraints: no file is held whole).
    #[tokio::test(flavor = "multi_thread")]
    async fn split_streams_a_20_mib_file_holding_under_one_mib() {
        let mut text = String::new();
        let mut n = 0u64;
        while text.len() < 20 << 20 {
            text.push_str(&format!("row-{n:08}\n"));
            n += 1;
        }
        for (cut, files) in [
            (json!({"rows": 200_000}), 9),
            (json!({"bytes": "4MB"}), 5),
            (json!({"partitions": 2}), 2),
        ] {
            let meter = Arc::new(Meter::new());
            let fs = store_with(meter.clone(), Caps::default());
            let mut index = Files::new();
            let from = stored(&fs, &mut index, "big.txt", text.as_bytes()).await;
            meter.read.store(0, Ordering::SeqCst);
            let mut spec = json!({ "from": from });
            spec.as_object_mut()
                .unwrap()
                .extend(cut.as_object().unwrap().clone());
            let t = task_over(SPLIT, spec, &fs, index);
            let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
            assert_eq!(out.state, RunState::Success, "{cut}: {out:?}");
            assert_eq!(out.files.len(), files, "{cut}");
            assert_eq!(
                meter.read.load(Ordering::SeqCst),
                text.len() as u64,
                "{cut}: one pass"
            );
            let peak = meter.peak.load(Ordering::SeqCst);
            eprintln!("{cut}: held at most {peak} bytes of {}", text.len());
            assert!(peak < 1 << 20, "{cut}: held {peak} bytes at once");
            let mut all = 0u64;
            for f in &out.files {
                all += f.size;
            }
            assert_eq!(all, text.len() as u64, "{cut}: every row written once");
        }
    }

    // ---- Split's cuts, Kestra's -----------------------------------------------

    async fn split(spec: serde_json::Value, body: &[u8], name: &str) -> (Arc<FileStore>, Outcome) {
        let fs = mem();
        let mut index = Files::new();
        let from = stored(&fs, &mut index, name, body).await;
        let mut spec = spec;
        spec["from"] = json!(from);
        let t = task_over(SPLIT, spec, &fs, index);
        let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
        (fs, out)
    }

    async fn parts(fs: &FileStore, out: &Outcome) -> Vec<String> {
        assert_eq!(out.state, RunState::Success, "{out:?}");
        let mut texts = Vec::new();
        for u in out.outputs.as_ref().unwrap()["uris"].as_array().unwrap() {
            let bytes = object(fs, u.as_str().unwrap()).await.unwrap();
            texts.push(String::from_utf8(bytes).unwrap());
        }
        texts
    }

    use serde_json::json;

    #[tokio::test(flavor = "multi_thread")]
    async fn split_cuts_as_kestra_does() {
        let lines = b"l1\nl2\nl3\nl4\nl5\n";
        let (fs, out) = split(json!({"rows": 2}), lines, "in.txt").await;
        assert_eq!(parts(&fs, &out).await, ["l1\nl2\n", "l3\nl4\n", "l5\n"]);
        let names: Vec<&str> = out
            .files
            .iter()
            .map(|f| f.uri.rsplit('/').next().unwrap())
            .collect();
        assert_eq!(
            names,
            ["part-00000.txt", "part-00001.txt", "part-00002.txt"]
        );
        let (fs, out) = split(json!({"partitions": 2}), lines, "in.txt").await;
        assert_eq!(parts(&fs, &out).await, ["l1\nl3\nl5\n", "l2\nl4\n"]);
        let (fs, out) = split(json!({"bytes": "6"}), lines, "in.txt").await;
        assert_eq!(parts(&fs, &out).await, ["l1\nl2\n", "l3\nl4\n", "l5\n"]);
        // Empty partitions are left out.
        let (fs, out) = split(json!({"partitions": 4}), b"a\nb\n", "in.txt").await;
        assert_eq!(parts(&fs, &out).await, ["a\n", "b\n"]);
        // Partitions past one pass's uploads: the order is the partition's.
        let body: String = (0..20).map(|i| format!("{i}\n")).collect();
        let (fs, out) = split(json!({"partitions": 10}), body.as_bytes(), "in.txt").await;
        let got = parts(&fs, &out).await;
        assert_eq!(got.len(), 10);
        assert_eq!(got[0], "0\n10\n");
        assert_eq!(got[9], "9\n19\n");
        // regexPattern: one file per first group, in order of first
        // appearance; a row that does not match is dropped.
        let (fs, out) = split(
            json!({"regexPattern": "^(\\w+):"}),
            b"b:1\na:2\nnone\nb:3\n",
            "in.log",
        )
        .await;
        assert_eq!(parts(&fs, &out).await, ["b:1\nb:3\n", "a:2\n"]);
        // Every first group, past one pass's uploads, in that order.
        let body: String = (0..20).map(|i| format!("k{i}:x\nk{i}:y\n")).collect();
        let (fs, out) = split(
            json!({"regexPattern": "^(\\w+):"}),
            body.as_bytes(),
            "in.txt",
        )
        .await;
        let got = parts(&fs, &out).await;
        assert_eq!(got.len(), 20);
        assert_eq!(got[0], "k0:x\nk0:y\n");
        assert_eq!(got[19], "k19:x\nk19:y\n");
        // A separator of its own, and a file with no extension.
        let (fs, out) = split(json!({"rows": 5, "separator": ";"}), b"a\r\nb", "noext").await;
        assert_eq!(parts(&fs, &out).await, ["a;b;"]);
        assert!(
            out.files[0].uri.ends_with("part-00000.tmp"),
            "{:?}",
            out.files
        );
        // None of the four: Kestra's message.
        let (_, out) = split(json!({}), lines, "in.txt").await;
        assert!(message(&out)
            .contains("Invalid configuration with no size, count, rows, nor regexPattern"));
    }

    /// A binary Ion file is split by VALUE (`IonSplitStrategy`), written as
    /// Ion text, one value per line.
    #[tokio::test(flavor = "multi_thread")]
    async fn split_of_binary_ion_splits_values() {
        let binary = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            // `core_storage`'s DeduplicateItems output: three binary values.
            "4AEA6umBg9aHtIFrgXbWioFiiyEC4AEA6umBg9aHtIFrgXbWioFhiyED4AEA6umBg9aHtIFrgXbWioFjiyEE",
        )
        .unwrap();
        let (fs, out) = split(json!({"rows": 2}), &binary, "in.ion").await;
        assert_eq!(
            parts(&fs, &out).await,
            ["{k:\"b\",v:2}\n{k:\"a\",v:3}\n", "{k:\"c\",v:4}\n"]
        );
    }

    /// The caps: a Split that would cross `max_task_bytes` fails naming it,
    /// and neither the file it was writing nor the outcome holds a file; one
    /// that would write more than [`MAX_SPLIT_FILES`] files is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn split_over_a_cap_fails_naming_it_and_keeps_nothing() {
        let fs = store_with(
            Arc::new(MemStore::new()),
            Caps {
                max_task_bytes: 10,
                ..Caps::default()
            },
        );
        let mut index = Files::new();
        let from = stored(&fs, &mut index, "in.txt", b"l1\nl2\nl3\nl4\nl5\n").await;
        let t = task_over(SPLIT, json!({"from": from, "rows": 2}), &fs, index.clone());
        let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
        assert!(
            message(&out).contains("workflow.storage.max_task_bytes"),
            "{out:?}"
        );
        assert!(out.files.is_empty());
        let third = format!("{}part-00001.txt", prefix());
        assert_eq!(
            object(&fs, &third).await,
            None,
            "the aborted upload stored nothing"
        );
        // The control: at the cap (the input's 15 bytes), it is written.
        let fs = store_with(
            Arc::new(MemStore::new()),
            Caps {
                max_task_bytes: 15,
                ..Caps::default()
            },
        );
        let mut index = Files::new();
        let from = stored(&fs, &mut index, "in.txt", b"l1\nl2\nl3\nl4\nl5\n").await;
        let t = task_over(SPLIT, json!({"from": from, "rows": 2}), &fs, index);
        let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
        assert_eq!(out.files.len(), 3, "{out:?}");

        let fs = mem();
        let mut index = Files::new();
        let body: String = (0..=MAX_SPLIT_FILES).map(|i| format!("{i}\n")).collect();
        let from = stored(&fs, &mut index, "many.txt", body.as_bytes()).await;
        let t = task_over(SPLIT, json!({"from": from, "rows": 1}), &fs, index.clone());
        let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
        assert!(
            message(&out).contains(&format!("more than {MAX_SPLIT_FILES} files")),
            "{out:?}"
        );
        let t = task_over(SPLIT, json!({"from": from, "rows": 2}), &fs, index);
        assert_eq!(
            Split::new(Some(fs.clone()))
                .run(&t)
                .await
                .unwrap()
                .files
                .len(),
            MAX_SPLIT_FILES / 2 + 1,
            "the control: half as many files"
        );
    }

    // ---- Concat, Reverse --------------------------------------------------------

    #[tokio::test(flavor = "multi_thread")]
    async fn concat_takes_a_list_or_a_json_string_and_appends_the_separator_after_each() {
        let fs = mem();
        let mut index = Files::new();
        let a = stored(&fs, &mut index, "a.txt", b"l1\nl2\n").await;
        let b = stored(&fs, &mut index, "b.tmp", b"o1\n").await;
        let t = task_over(
            CONCAT,
            json!({"files": [a, b], "separator": "--\n"}),
            &fs,
            index.clone(),
        );
        let out = Concat::new(Some(fs.clone())).run(&t).await.unwrap();
        assert_eq!(text_of(&fs, &out).await, "l1\nl2\n--\no1\n--\n");
        assert!(out.files[0].uri.ends_with("/content.tmp"));
        let list = serde_json::to_string(&json!([a, b])).unwrap();
        let t = task_over(
            CONCAT,
            json!({"files": list, "extension": ".txt"}),
            &fs,
            index.clone(),
        );
        let out = Concat::new(Some(fs.clone())).run(&t).await.unwrap();
        assert_eq!(text_of(&fs, &out).await, "l1\nl2\no1\n");
        assert!(out.files[0].uri.ends_with("/content.txt"));
        let t = task_over(CONCAT, json!({"files": 3}), &fs, index);
        assert!(message(&Concat::new(Some(fs)).run(&t).await.unwrap()).contains("Invalid `files`"));
    }

    #[test]
    fn reverse_rows_are_reversed_lines_file_readers() {
        let rows = |b: &[u8]| -> Vec<String> {
            reversed_rows(b)
                .into_iter()
                .map(|r| String::from_utf8(r.to_vec()).unwrap())
                .collect()
        };
        assert_eq!(rows(b"l1\nl2\nl3\n"), ["l3", "l2", "l1"]);
        assert_eq!(rows(b"l1\nl2"), ["l2", "l1"]);
        assert_eq!(rows(b"a\r\nb\rc"), ["c", "b", "a"]);
        assert_eq!(rows(b"a\n\n"), ["", "a"]);
        assert_eq!(rows(b"\n"), [""]);
        assert!(rows(b"").is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reverse_writes_rows_last_first_and_refuses_over_the_read_cap() {
        let fs = store_with(
            Arc::new(MemStore::new()),
            Caps {
                max_read_bytes: 15,
                ..Caps::default()
            },
        );
        let mut index = Files::new();
        let from = stored(&fs, &mut index, "in.txt", b"l1\nl2\nl3\nl4\nl5\n").await;
        let t = task_over(REVERSE, json!({ "from": from }), &fs, index.clone());
        let out = Reverse::new(Some(fs.clone())).run(&t).await.unwrap();
        assert_eq!(text_of(&fs, &out).await, "l5\nl4\nl3\nl2\nl1\n");
        assert!(out.files[0].uri.ends_with("/content.txt"));
        let big = stored(&fs, &mut index, "big.txt", b"0123456789abcdef").await;
        let t = task_over(REVERSE, json!({ "from": big }), &fs, index.clone());
        let m = Reverse::new(Some(fs.clone())).run(&t).await.unwrap();
        assert!(message(&m).contains("max_read_bytes (15 bytes)"), "{m:?}");
        let t = task_over(
            REVERSE,
            json!({ "from": from, "charset": "ISO-8859-1" }),
            &fs,
            index,
        );
        assert!(message(&Reverse::new(Some(fs)).run(&t).await.unwrap()).contains("UTF-8"));
    }

    // ---- The item tasks -----------------------------------------------------------

    async fn items_task(
        type_id: &str,
        spec: serde_json::Value,
        body: &[u8],
    ) -> (Arc<FileStore>, Outcome) {
        let fs = mem();
        let mut index = Files::new();
        let from = stored(&fs, &mut index, "in.ion", body).await;
        let mut spec = spec;
        spec["from"] = json!(from);
        let t = task_over(type_id, spec, &fs, index);
        let out = match type_id {
            FILTER_ITEMS => FilterItems::new(Some(fs.clone())).run(&t).await,
            _ => DeduplicateItems::new(Some(fs.clone())).run(&t).await,
        }
        .unwrap();
        (fs, out)
    }

    async fn kept(fs: &FileStore, out: &Outcome) -> Vec<serde_json::Value> {
        items_of(text_of(fs, out).await.as_bytes())
    }

    /// The LAST item of each key, in their order; the item's fields are the
    /// expression's top-level names.
    #[tokio::test(flavor = "multi_thread")]
    async fn deduplicate_keeps_the_last_item_of_each_key() {
        let (fs, out) = items_task(
            DEDUPLICATE_ITEMS,
            json!({"expr": "{{ k }}"}),
            ITEMS.as_bytes(),
        )
        .await;
        assert_eq!(
            kept(&fs, &out).await,
            [
                json!({"k":"b","v":2}),
                json!({"k":"a","v":3}),
                json!({"k":"c","v":4})
            ]
        );
        let o = out.outputs.unwrap();
        assert_eq!(
            (
                &o["numKeys"],
                &o["processedItemsTotal"],
                &o["droppedItemsTotal"]
            ),
            (&json!(3), &json!(4), &json!(1))
        );
        // An expression that fails on an item fails the task, naming it.
        let (_, out) = items_task(
            DEDUPLICATE_ITEMS,
            json!({"expr": "{{ nope }}"}),
            ITEMS.as_bytes(),
        )
        .await;
        assert!(message(&out).contains("`expr` failed on item 0"), "{out:?}");
    }

    /// `INCLUDE` keeps the truthy items, `EXCLUDE` drops them (the fixture's
    /// two runs), and `TruthUtils`' four falsy texts are false.
    #[tokio::test(flavor = "multi_thread")]
    async fn filter_includes_or_excludes_what_the_condition_holds() {
        let (fs, out) = items_task(
            FILTER_ITEMS,
            json!({"filterCondition": "{{ v > 1 }}"}),
            ITEMS.as_bytes(),
        )
        .await;
        assert_eq!(kept(&fs, &out).await.len(), 3);
        assert_eq!(out.outputs.as_ref().unwrap()["droppedItemsTotal"], json!(1));
        let (fs, out) = items_task(
            FILTER_ITEMS,
            json!({"filterCondition": "{{ k == 'a' }}", "filterType": "EXCLUDE"}),
            ITEMS.as_bytes(),
        )
        .await;
        assert_eq!(
            kept(&fs, &out).await,
            [json!({"k":"b","v":2}), json!({"k":"c","v":4})]
        );
        for falsy in ["false", "0", "-0", "", " false "] {
            assert!(!truthy(falsy), "{falsy:?}");
        }
        for t in ["true", "1", "no", "False"] {
            assert!(truthy(t), "{t:?}");
        }
    }

    /// An item whose condition FAILS follows `errorOrNullBehavior`, each
    /// value: `FAIL` fails the task (a failed task, never `Err`), `INCLUDE`
    /// keeps the item, `EXCLUDE` drops it -- with `filterType` either way, as
    /// the behaviour is the action itself.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_condition_follows_error_or_null_behavior() {
        // `x` exists on the second item only: the others fail.
        let body = "{v:1}\n{v:2,x:true}\n{v:3}";
        for filter_type in ["INCLUDE", "EXCLUDE"] {
            let spec = |b: &str| json!({"filterCondition": "{{ x }}", "filterType": filter_type, "errorOrNullBehavior": b});
            let (_, out) = items_task(FILTER_ITEMS, spec("FAIL"), body.as_bytes()).await;
            assert!(
                message(&out).contains("`filterCondition` failed on item 0"),
                "{out:?}"
            );
            let (fs, out) = items_task(FILTER_ITEMS, spec("INCLUDE"), body.as_bytes()).await;
            let want_include: Vec<_> = if filter_type == "INCLUDE" {
                vec![json!({"v":1}), json!({"v":2,"x":true}), json!({"v":3})]
            } else {
                vec![json!({"v":1}), json!({"v":3})]
            };
            assert_eq!(
                kept(&fs, &out).await,
                want_include,
                "{filter_type} / INCLUDE"
            );
            let (fs, out) = items_task(FILTER_ITEMS, spec("EXCLUDE"), body.as_bytes()).await;
            let want_exclude: Vec<_> = if filter_type == "INCLUDE" {
                vec![json!({"v":2,"x":true})]
            } else {
                vec![]
            };
            assert_eq!(
                kept(&fs, &out).await,
                want_exclude,
                "{filter_type} / EXCLUDE"
            );
        }
        assert_eq!(
            decide(Keep::Include, None, Ok("x".into())),
            Ok(Keep::Include)
        );
        assert_eq!(
            decide(Keep::Exclude, None, Ok("0".into())),
            Ok(Keep::Include)
        );
        assert_eq!(
            decide(Keep::Exclude, Some(Keep::Include), Err("e".into())),
            Ok(Keep::Include)
        );
        assert_eq!(
            decide(Keep::Include, None, Err("e".into())),
            Err("e".into())
        );
    }

    /// The item is bound OVER the run's variables (its `flow` hides the run's),
    /// a `null` field bound as `null` -- `{{ n }}` renders, where an unbound
    /// name would fail the condition and, under `FAIL`, the task; an item
    /// that is not a struct fails the task.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_item_is_bound_over_the_runs_variables() {
        let fs = mem();
        let mut index = Files::new();
        let from = stored(
            &fs,
            &mut index,
            "in.ion",
            b"{flow:\"item\",n:null}\n{flow:\"other\",n:null}",
        )
        .await;
        let mut t = task_over(
            FILTER_ITEMS,
            json!({"from": from, "filterCondition": "{{ n }}{{ flow == 'item' }}"}),
            &fs,
            index.clone(),
        );
        let files = t.render.files().cloned().unwrap();
        let mut cx = RenderContext::new();
        cx.insert("flow", json!({"id": "f"}));
        t.render = cx.with_files(files);
        let out = FilterItems::new(Some(fs.clone())).run(&t).await.unwrap();
        assert_eq!(kept(&fs, &out).await, [json!({"flow":"item"})]);

        let from = stored(&fs, &mut index, "scalar.ion", b"1 2").await;
        let t = task_over(
            FILTER_ITEMS,
            json!({"from": from, "filterCondition": "{{ true }}", "errorOrNullBehavior": "INCLUDE"}),
            &fs,
            index,
        );
        let out = FilterItems::new(Some(fs)).run(&t).await.unwrap();
        assert!(message(&out).contains("is not a record"), "{out:?}");
    }

    /// No way around the render's rules from inside a file: `secret()` is
    /// refused, an item's `{{ … }}` text is not evaluated (one pass), and a
    /// URI an item names is not in the run's snapshot, so `read()` refuses
    /// it -- each a failed condition, which `EXCLUDE` here turns into a
    /// dropped item and `FAIL` into the render's own refusal.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_condition_cannot_reach_a_secret_or_a_file_the_run_cannot_read() {
        let fs = mem();
        let mut index = Files::new();
        let secret_file = stored(&fs, &mut index, "secret.txt", b"s3cr3t").await;
        let from = stored(
            &fs,
            &mut index,
            "in.ion",
            format!("{{t:\"{{{{ 0 }}}}\",u:\"{}\"}}", input_uri("unindexed.txt")).as_bytes(),
        )
        .await;
        let _ = secret_file;
        let run = |cond: &str, b: &str| {
            task_over(
                FILTER_ITEMS,
                json!({"from": from, "filterCondition": cond, "errorOrNullBehavior": b}),
                &fs,
                index.clone(),
            )
        };
        let out = FilterItems::new(Some(fs.clone()))
            .run(&run("{{ secret('x') }}", "FAIL"))
            .await
            .unwrap();
        assert!(
            message(&out).contains(crate::runners::SECRET_REFUSAL),
            "{out:?}"
        );
        let out = FilterItems::new(Some(fs.clone()))
            .run(&run("{{ read(u) }}", "FAIL"))
            .await
            .unwrap();
        assert!(message(&out).contains("was not looked up"), "{out:?}");
        // One pass: `{{ t }}` is the item's text `{{ 0 }}`, truthy, and not
        // a second render's `0`, which is falsy.
        let out = FilterItems::new(Some(fs.clone()))
            .run(&run("{{ t }}", "FAIL"))
            .await
            .unwrap();
        assert_eq!(
            out.outputs.as_ref().unwrap()["droppedItemsTotal"],
            json!(0),
            "{out:?}"
        );
    }

    // ---- Outages are the worker's -----------------------------------------------

    /// A store whose `get_stream` (or `put_stream`) fails as S3 unreachable
    /// does.
    struct Down {
        inner: MemStore,
        reads: AtomicBool,
    }

    #[async_trait::async_trait]
    impl ObjectStore for Down {
        async fn put(&self, uri: &str, bytes: Vec<u8>) -> std::result::Result<(), StorageError> {
            self.inner.put(uri, bytes).await
        }
        async fn get(&self, uri: &str) -> std::result::Result<Vec<u8>, StorageError> {
            self.inner.get(uri).await
        }
        async fn delete(&self, _: &str) -> std::result::Result<(), StorageError> {
            Ok(())
        }
        async fn copy(&self, _: &str, _: &str) -> std::result::Result<(), StorageError> {
            Ok(())
        }
        async fn list(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, StorageError> {
            unreachable!()
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> std::result::Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError>
        {
            if self.reads.load(Ordering::SeqCst) {
                return self.inner.get_stream(uri).await;
            }
            Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )))
        }
        async fn put_stream(
            &self,
            uri: &str,
            _: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            _: Option<u64>,
        ) -> std::result::Result<u64, StorageError> {
            Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )))
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_store_outage_reading_or_writing_is_a_worker_failure() {
        for reads in [false, true] {
            let fs = store_with(
                Arc::new(Down {
                    inner: MemStore::new(),
                    reads: AtomicBool::new(reads),
                }),
                Caps::default(),
            );
            let mut index = Files::new();
            let from = stored(&fs, &mut index, "in.ion", ITEMS.as_bytes()).await;
            for (type_id, spec) in [
                (SPLIT, json!({"from": from, "rows": 1})),
                (REVERSE, json!({"from": from})),
                (CONCAT, json!({"files": [from]})),
                (
                    FILTER_ITEMS,
                    json!({"from": from, "filterCondition": "{{ true }}"}),
                ),
                (DEDUPLICATE_ITEMS, json!({"from": from, "expr": "{{ k }}"})),
            ] {
                let t = task_over(type_id, spec, &fs, index.clone());
                let handler: Box<dyn TaskHandler> = match type_id {
                    SPLIT => Box::new(Split::new(Some(fs.clone()))),
                    REVERSE => Box::new(Reverse::new(Some(fs.clone()))),
                    CONCAT => Box::new(Concat::new(Some(fs.clone()))),
                    FILTER_ITEMS => Box::new(FilterItems::new(Some(fs.clone()))),
                    _ => Box::new(DeduplicateItems::new(Some(fs.clone()))),
                };
                let err = handler
                    .run(&t)
                    .await
                    .expect_err("an outage is Err, not a failed task");
                assert!(
                    err.to_string().contains("internal storage is unavailable"),
                    "{type_id} reads={reads}: {err}"
                );
            }
        }
    }

    /// `MemStore` whose `get_stream` fails, as S3 unreachable does, for an
    /// object whose key holds `outage`.
    struct DownFor(MemStore);

    #[async_trait::async_trait]
    impl ObjectStore for DownFor {
        async fn put(&self, uri: &str, bytes: Vec<u8>) -> std::result::Result<(), StorageError> {
            self.0.put(uri, bytes).await
        }
        async fn get(&self, uri: &str) -> std::result::Result<Vec<u8>, StorageError> {
            self.0.get(uri).await
        }
        async fn delete(&self, uri: &str) -> std::result::Result<(), StorageError> {
            self.0.delete(uri).await
        }
        async fn copy(&self, a: &str, b: &str) -> std::result::Result<(), StorageError> {
            self.0.copy(a, b).await
        }
        async fn list(
            &self,
            p: &str,
            d: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, StorageError> {
            self.0.list(p, d).await
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> std::result::Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError>
        {
            if uri.contains("outage") {
                return Err(StorageError::S3(format!(
                    "{uri}: dispatch failure: connection refused"
                )));
            }
            self.0.get_stream(uri).await
        }
        async fn put_stream(
            &self,
            uri: &str,
            body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            hint: Option<u64>,
        ) -> std::result::Result<u64, StorageError> {
            self.0.put_stream(uri, body, hint).await
        }
        async fn head(&self, uri: &str) -> std::result::Result<Option<u64>, StorageError> {
            self.0.head(uri).await
        }
    }

    /// A `read()` inside an item's expression that meets a store OUTAGE is
    /// the worker's (spec §11), not a failed condition that
    /// `errorOrNullBehavior: EXCLUDE` would quietly turn into a dropped item.
    /// The control: the same condition over a readable file keeps the item.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_outage_inside_an_items_render_is_a_worker_failure() {
        let fs = store_with(Arc::new(DownFor(MemStore::new())), Caps::default());
        let mut index = Files::new();
        let from = stored(&fs, &mut index, "in.ion", b"{v:1}").await;
        let down = stored(&fs, &mut index, "outage.txt", b"x").await;
        let up = stored(&fs, &mut index, "up.txt", b"x").await;
        for (type_id, key) in [
            (FILTER_ITEMS, "filterCondition"),
            (DEDUPLICATE_ITEMS, "expr"),
        ] {
            let spec = |file: &str| {
                let mut spec = json!({"from": from, "errorOrNullBehavior": "EXCLUDE"});
                spec[key] = json!(format!("{{{{ read('{file}') }}}}"));
                spec
            };
            let handler = |fs: &Arc<FileStore>| -> Box<dyn TaskHandler> {
                match type_id {
                    FILTER_ITEMS => Box::new(FilterItems::new(Some(fs.clone()))),
                    _ => Box::new(DeduplicateItems::new(Some(fs.clone()))),
                }
            };
            let t = task_over(type_id, spec(&down), &fs, index.clone());
            let err = handler(&fs).run(&t).await.expect_err("an outage is Err");
            assert!(
                err.to_string().contains("internal storage is unavailable"),
                "{type_id}: {err}"
            );
            let t = task_over(type_id, spec(&up), &fs, index.clone());
            let out = handler(&fs).run(&t).await.unwrap();
            assert_eq!(out.state, RunState::Success, "{type_id}: {out:?}");
        }
    }

    /// An index row whose object is gone, or holds other bytes than it says,
    /// is the author's error naming the URI.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_object_that_disagrees_with_its_row_is_the_authors_error() {
        let fs = mem();
        let mut index = Files::new();
        let from = stored(&fs, &mut index, "in.txt", b"l1\nl2\n").await;
        let mut short = index.clone();
        if let Some(Ok(f)) = short.get_mut(&from) {
            f.size = 99;
        }
        let t = task_over(SPLIT, json!({"from": from, "rows": 1}), &fs, short);
        assert!(
            message(&Split::new(Some(fs.clone())).run(&t).await.unwrap()).contains("holds only 6")
        );
        let mut long = index.clone();
        if let Some(Ok(f)) = long.get_mut(&from) {
            f.size = 2;
        }
        let t = task_over(SPLIT, json!({"from": from, "rows": 1}), &fs, long);
        assert!(
            message(&Split::new(Some(fs.clone())).run(&t).await.unwrap()).contains("holds more")
        );
        fs.store
            .delete(&fs.object_uri(Uuid::nil(), &from).unwrap())
            .await
            .unwrap();
        let t = task_over(SPLIT, json!({"from": from, "rows": 1}), &fs, index);
        assert!(
            message(&Split::new(Some(fs)).run(&t).await.unwrap()).contains("missing from storage")
        );
    }

    /// A URI the snapshot refuses is refused, naming it, before any read.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_unreadable_input_is_refused_as_read_refuses_it() {
        let fs = mem();
        let foreign = format!(
            "{}x.txt",
            uri::task_prefix("w9a.t12", "unit", Uuid::from_u128(0xee), "w", WRITER).unwrap()
        );
        let index = Files::from([(
            foreign.clone(),
            Err(Unreadable::NotReadable {
                uri: foreign.clone(),
            }),
        )]);
        let t = task_over(
            SPLIT,
            json!({"from": foreign, "rows": 1}),
            &fs,
            index.clone(),
        );
        assert!(
            message(&Split::new(Some(fs.clone())).run(&t).await.unwrap()).contains("may not read")
        );
        let t = task_over(CONCAT, json!({"files": [foreign]}), &fs, index);
        assert!(message(&Concat::new(Some(fs)).run(&t).await.unwrap()).contains("may not read"));
    }

    // ---- W9a Task 12 fix round 1 ------------------------------------------------

    /// `MemStore` with two faults on demand: `get_stream` of a key holding
    /// `breaks` yields `after` bytes and then fails as a dropped connection
    /// does; `get_stream` of a key holding `flips` serves `second` from its
    /// second read on; `put_stream` of a key holding `stalls` never ends.
    struct Tricky {
        inner: MemStore,
        after: usize,
        second: Vec<u8>,
        flips_read: AtomicU64,
    }

    impl Tricky {
        fn new(after: usize, second: &[u8]) -> Self {
            Self {
                inner: MemStore::new(),
                after,
                second: second.to_vec(),
                flips_read: AtomicU64::new(0),
            }
        }
    }

    /// Hands over `left` bytes of `inner`, then fails.
    struct BreaksAfter {
        inner: Box<dyn tokio::io::AsyncBufRead + Send + Unpin>,
        left: usize,
    }

    impl AsyncRead for BreaksAfter {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.left == 0 {
                return std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "connection reset by peer",
                )));
            }
            let take = self.left.min(buf.remaining());
            let mut limited = ReadBuf::new(buf.initialize_unfilled_to(take));
            let poll = std::pin::Pin::new(&mut self.inner).poll_read(cx, &mut limited);
            let n = limited.filled().len();
            buf.advance(n);
            self.left -= n;
            poll
        }
    }

    #[async_trait::async_trait]
    impl ObjectStore for Tricky {
        async fn put(&self, uri: &str, bytes: Vec<u8>) -> std::result::Result<(), StorageError> {
            self.inner.put(uri, bytes).await
        }
        async fn get(&self, uri: &str) -> std::result::Result<Vec<u8>, StorageError> {
            self.inner.get(uri).await
        }
        async fn delete(&self, uri: &str) -> std::result::Result<(), StorageError> {
            self.inner.delete(uri).await
        }
        async fn copy(&self, a: &str, b: &str) -> std::result::Result<(), StorageError> {
            self.inner.copy(a, b).await
        }
        async fn list(
            &self,
            p: &str,
            d: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, StorageError> {
            self.inner.list(p, d).await
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> std::result::Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError>
        {
            if uri.contains("flips") && self.flips_read.fetch_add(1, Ordering::SeqCst) > 0 {
                return Ok(Box::new(std::io::Cursor::new(self.second.clone())));
            }
            let inner = self.inner.get_stream(uri).await?;
            if uri.contains("breaks") {
                return Ok(Box::new(tokio::io::BufReader::new(BreaksAfter {
                    inner,
                    left: self.after,
                })));
            }
            Ok(inner)
        }
        async fn put_stream(
            &self,
            uri: &str,
            body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            hint: Option<u64>,
        ) -> std::result::Result<u64, StorageError> {
            if uri.contains("stalls") {
                std::future::pending::<()>().await;
            }
            self.inner.put_stream(uri, body, hint).await
        }
        async fn head(&self, uri: &str) -> std::result::Result<Option<u64>, StorageError> {
            self.inner.head(uri).await
        }
    }

    fn handler_for(type_id: &str, fs: &Arc<FileStore>) -> Box<dyn TaskHandler> {
        match type_id {
            SPLIT => Box::new(Split::new(Some(fs.clone()))),
            REVERSE => Box::new(Reverse::new(Some(fs.clone()))),
            CONCAT => Box::new(Concat::new(Some(fs.clone()))),
            FILTER_ITEMS => Box::new(FilterItems::new(Some(fs.clone()))),
            _ => Box::new(DeduplicateItems::new(Some(fs.clone()))),
        }
    }

    /// I3 (d): a store that fails IN THE MIDDLE of a read -- after the
    /// object opened and some bytes arrived -- is an outage, the worker's
    /// (spec §11), for every one of the five; the control reads the same
    /// object whole.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_read_that_fails_mid_stream_is_a_worker_failure() {
        let fs = store_with(Arc::new(Tricky::new(7, b"")), Caps::default());
        let mut index = Files::new();
        let breaks = stored(&fs, &mut index, "breaks.ion", ITEMS.as_bytes()).await;
        let whole = stored(&fs, &mut index, "whole.ion", ITEMS.as_bytes()).await;
        for from in [&breaks, &whole] {
            for (type_id, spec) in [
                (SPLIT, json!({"from": from, "rows": 1})),
                (REVERSE, json!({"from": from})),
                (CONCAT, json!({"files": [from]})),
                (
                    FILTER_ITEMS,
                    json!({"from": from, "filterCondition": "{{ true }}"}),
                ),
                (DEDUPLICATE_ITEMS, json!({"from": from, "expr": "{{ k }}"})),
            ] {
                let t = task_over(type_id, spec, &fs, index.clone());
                let out = handler_for(type_id, &fs).run(&t).await;
                if from == &whole {
                    assert_eq!(out.unwrap().state, RunState::Success, "{type_id}: control");
                    continue;
                }
                let err = out.expect_err("a store fault mid-read is Err, not a failed task");
                assert!(
                    err.to_string().contains("internal storage is unavailable"),
                    "{type_id}: {err}"
                );
            }
        }
    }

    /// I3 (e): an object that holds a different NUMBER of items on
    /// DeduplicateItems' second read than on its first (same size, other
    /// bytes) is not written as if it were the same file -- more items, or
    /// fewer, is a worker failure naming it.
    #[tokio::test(flavor = "multi_thread")]
    async fn deduplicate_refuses_an_object_that_changed_between_its_reads() {
        for (first, second) in [
            (&b"{k:1} {k:2} "[..], &b"{k:1}{k:2}{}"[..]),
            (&b"{k:1}{k:2}{}"[..], &b"{k:1} {k:2} "[..]),
        ] {
            let fs = store_with(Arc::new(Tricky::new(0, second)), Caps::default());
            let mut index = Files::new();
            let from = stored(&fs, &mut index, "flips.ion", first).await;
            let t = task_over(
                DEDUPLICATE_ITEMS,
                json!({"from": from, "expr": "{{ 1 }}"}),
                &fs,
                index,
            );
            let err = DeduplicateItems::new(Some(fs.clone()))
                .run(&t)
                .await
                .expect_err("a changed object is Err");
            assert!(err.to_string().contains("second read"), "{err}");
        }
    }

    /// I2: DeduplicateItems' cap bounds what it HOLDS -- every distinct key
    /// costs its bytes and a fixed overhead -- so a hundred one-byte keys do
    /// not pass a 1000-byte cap they would pass counting key bytes alone
    /// (about 190); ten do. The refusal names the cap.
    #[tokio::test(flavor = "multi_thread")]
    async fn deduplicate_charges_each_key_what_it_holds() {
        let caps = Caps {
            max_read_bytes: 1000,
            ..Caps::default()
        };
        for (keys, refused) in [(100, true), (10, false)] {
            let fs = store_with(Arc::new(MemStore::new()), caps);
            let mut index = Files::new();
            let body: String = (0..keys).map(|i| format!("{{k:{i}}}\n")).collect();
            let from = stored(&fs, &mut index, "in.ion", body.as_bytes()).await;
            let t = task_over(
                DEDUPLICATE_ITEMS,
                json!({"from": from, "expr": "{{ k }}"}),
                &fs,
                index,
            );
            let out = DeduplicateItems::new(Some(fs.clone()))
                .run(&t)
                .await
                .unwrap();
            if refused {
                assert!(
                    message(&out).contains("max_read_bytes (1000 bytes)"),
                    "{out:?}"
                );
            } else {
                assert_eq!(
                    out.outputs.as_ref().unwrap()["numKeys"],
                    json!(keys),
                    "{out:?}"
                );
            }
        }
    }

    /// I1: an item over `max_read_bytes` -- one value is held whole -- fails
    /// the task naming the cap; the items before it are not a success.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_item_over_the_read_cap_fails_naming_it() {
        let fs = store_with(
            Arc::new(MemStore::new()),
            Caps {
                max_read_bytes: 64 << 10,
                ..Caps::default()
            },
        );
        let mut index = Files::new();
        // Each item under the cap but one: a value of twice it.
        let body = format!(
            "{}{{s:\"{}\"}}\n",
            "{v:1}\n".repeat(20_000),
            "x".repeat(128 << 10)
        );
        let from = stored(&fs, &mut index, "in.ion", body.as_bytes()).await;
        for (type_id, spec) in [
            (
                FILTER_ITEMS,
                json!({"from": from, "filterCondition": "{{ true }}"}),
            ),
            (DEDUPLICATE_ITEMS, json!({"from": from, "expr": "{{ 1 }}"})),
        ] {
            let t = task_over(type_id, spec, &fs, index.clone());
            let out = handler_for(type_id, &fs).run(&t).await.unwrap();
            assert!(
                message(&out).contains("max_read_bytes (65536 bytes)"),
                "{type_id}: {out:?}"
            );
        }
    }

    /// I3 (c): `partitions` under 1 or over [`MAX_SPLIT_FILES`] is refused
    /// before a byte is read, naming the bound; at the bound it is accepted.
    #[tokio::test(flavor = "multi_thread")]
    async fn partitions_out_of_range_are_refused() {
        for (n, says) in [
            (0i64, "at least 1".to_string()),
            (-1, "at least 1".to_string()),
            (
                MAX_SPLIT_FILES as i64 + 1,
                format!("at most {MAX_SPLIT_FILES} files"),
            ),
        ] {
            let (_, out) = split(json!({"partitions": n}), b"a\nb\n", "in.txt").await;
            assert!(message(&out).contains(&says), "{n}: {out:?}");
        }
        let (_, out) = split(json!({"partitions": MAX_SPLIT_FILES}), b"a\nb\n", "in.txt").await;
        assert_eq!(out.state, RunState::Success, "{out:?}");
    }

    /// I3 (b): a keyed Split that meets more keys than it may write files is
    /// refused naming the limit (`split_keyed`'s own check; production
    /// passes [`MAX_SPLIT_FILES`]), and one at the limit is written.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_keyed_split_past_its_file_limit_is_refused() {
        let fs = mem();
        let mut index = Files::new();
        let from = stored(&fs, &mut index, "in.txt", b"a:1\nb:2\nc:3\n").await;
        for (max, ok) in [(2usize, false), (3, true)] {
            let run = Run::new(fs.clone(), Uuid::nil(), prefix());
            let cut = Cut::Regex(crate::runners::compile_java_regex("^(\\w+):").unwrap());
            let from = from.clone();
            let got = run_blocking(run, move |run| {
                split_keyed(run, &from, 12, "\n", ".txt", &cut, max)
            })
            .await;
            match (ok, got) {
                (true, Ok(files)) => assert_eq!(files.len(), 3),
                (false, Err(Halt::Author(m))) => {
                    assert!(m.contains("more than 2 files"), "{m}")
                }
                (_, other) => panic!("max {max}: {other:?}"),
            }
        }
    }

    /// I4: a Split by `partitions` that would read its input more than
    /// `max_task_bytes` in all -- `ceil(partitions / 8)` passes -- is refused
    /// UP FRONT, before a byte is read, naming the cap; at the cap it runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_partitions_split_over_the_read_budget_is_refused_up_front() {
        let body: String = (0..20).map(|i| format!("{i:02}\n")).collect(); // 60 bytes
        for (budget, refused) in [(119u64, true), (120, false)] {
            let meter = Arc::new(Meter::new());
            let fs = store_with(
                meter.clone(),
                Caps {
                    max_task_bytes: budget,
                    ..Caps::default()
                },
            );
            let mut index = Files::new();
            let from = stored(&fs, &mut index, "in.txt", body.as_bytes()).await;
            meter.read.store(0, Ordering::SeqCst);
            let t = task_over(SPLIT, json!({"from": from, "partitions": 16}), &fs, index);
            let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
            if refused {
                assert_eq!(meter.read.load(Ordering::SeqCst), 0, "read before refusing");
                assert!(
                    message(&out).contains("workflow.storage.max_task_bytes"),
                    "{out:?}"
                );
                assert!(message(&out).contains("2 passes"), "{out:?}");
            } else {
                assert_eq!(out.files.len(), 16, "{out:?}");
            }
        }
    }

    /// I4: a Split by `regexPattern` counts its passes against the same
    /// budget as they run: nine keys need two passes, refused under twice
    /// the input and written at it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_regex_split_over_the_read_budget_is_refused_as_it_runs() {
        let body: String = (0..9).map(|i| format!("k{i}:x\n")).collect(); // 45 bytes
        for (budget, refused) in [(89u64, true), (90, false)] {
            let fs = store_with(
                Arc::new(MemStore::new()),
                Caps {
                    max_task_bytes: budget,
                    ..Caps::default()
                },
            );
            let mut index = Files::new();
            let from = stored(&fs, &mut index, "in.txt", body.as_bytes()).await;
            let t = task_over(
                SPLIT,
                json!({"from": from, "regexPattern": "^(\\w+):"}),
                &fs,
                index,
            );
            let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
            if refused {
                assert!(
                    message(&out).contains("workflow.storage.max_task_bytes"),
                    "{out:?}"
                );
            } else {
                assert_eq!(out.files.len(), 9, "{out:?}");
            }
        }
    }

    /// I4 + M4: no pass is read for nothing -- `partitions: 16` over three
    /// rows, and a `regexPattern` with exactly eight keys, each read their
    /// input ONCE.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_keyed_split_reads_no_pass_it_does_not_need() {
        let eight: String = (0..8).map(|i| format!("k{i}:x\n")).collect();
        for (cut, body, files) in [
            (json!({"partitions": 16}), "a\nb\nc\n".to_string(), 3),
            (json!({"regexPattern": "^(\\w+):"}), eight, 8),
        ] {
            let meter = Arc::new(Meter::new());
            let fs = store_with(meter.clone(), Caps::default());
            let mut index = Files::new();
            let from = stored(&fs, &mut index, "in.txt", body.as_bytes()).await;
            meter.read.store(0, Ordering::SeqCst);
            let mut spec = cut.clone();
            spec["from"] = json!(from);
            let t = task_over(SPLIT, spec, &fs, index);
            let out = Split::new(Some(fs.clone())).run(&t).await.unwrap();
            assert_eq!(out.files.len(), files, "{cut}: {out:?}");
            assert_eq!(
                meter.read.load(Ordering::SeqCst),
                body.len() as u64,
                "{cut}: one pass"
            );
        }
    }

    /// M5: `TruthUtils` trims as Java's `String.trim` does -- every char at
    /// or under U+0020 and nothing else -- so a no-break space is kept (and
    /// the text is truthy) while a control character is trimmed.
    #[test]
    fn truthy_trims_as_java_does() {
        assert!(truthy("\u{a0}0"), "U+00A0 is not trimmed by Java");
        assert!(truthy("\u{3000}false"), "U+3000 is not trimmed by Java");
        assert!(!truthy("\u{1}0\u{1f}"), "controls are trimmed by Java");
        assert!(!truthy("\t false \n"));
    }

    /// M6: an item field named like a global the render binds for itself
    /// (`egret file scope`, which no template can name: it holds a space)
    /// does not hide it -- `fileExists` still reads the run's scope. The
    /// control is the same item without that field.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_item_field_cannot_hide_the_renders_own_globals() {
        let fs = mem();
        let mut index = Files::new();
        let up = stored(&fs, &mut index, "up.txt", b"x").await;
        for body in [
            "{'egret file scope':1,'egret context scope':2,v:1}",
            "{v:1}",
        ] {
            let from = stored(&fs, &mut index, "in.ion", body.as_bytes()).await;
            let t = task_over(
                FILTER_ITEMS,
                json!({"from": from, "filterCondition": format!("{{{{ fileExists('{up}') }}}}")}),
                &fs,
                index.clone(),
            );
            let out = FilterItems::new(Some(fs.clone())).run(&t).await.unwrap();
            assert_eq!(out.state, RunState::Success, "{body}: {out:?}");
            assert_eq!(
                out.outputs.as_ref().unwrap()["droppedItemsTotal"],
                json!(0),
                "{body}"
            );
        }
    }

    /// M7: dropping an unfinished upload whose store has stalled -- its
    /// `put_stream` never ends -- returns after the abort wait instead of
    /// holding the thread for ever.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_aborted_upload_does_not_wait_for_a_stalled_store() {
        let fs = store_with(Arc::new(Tricky::new(0, b"")), Caps::default());
        let run = Run::new(fs, Uuid::nil(), prefix())
            .with_abort_wait(std::time::Duration::from_millis(200));
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = tokio::runtime::Handle::current();
        // A plain thread, so a hang fails this test instead of hanging it.
        std::thread::spawn(move || {
            let _guard = handle.enter();
            let done = handle.block_on(run_blocking(run, |run| {
                let mut upload = run.create("stalls.txt")?;
                upload.write(b"some bytes")?;
                drop(upload);
                Ok(())
            }));
            let _ = tx.send(done);
        });
        let done = tokio::task::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(20))
        })
        .await
        .unwrap();
        assert_eq!(done, Ok(Ok(())), "the drop returned");
    }

    #[test]
    fn bytes_and_extensions_parse_as_kestras() {
        assert_eq!(parse_bytes("6"), Some(6));
        assert_eq!(parse_bytes("2KB"), Some(2048));
        assert_eq!(parse_bytes("1mb"), Some(1 << 20));
        assert_eq!(parse_bytes("1GB"), Some(1 << 30));
        assert_eq!(parse_bytes("6 bytes"), None);
        assert_eq!(split_extension("kestra:///a/b/x.txt").unwrap(), ".txt");
        assert_eq!(split_extension("kestra:///a/b/x").unwrap(), ".tmp");
        assert!(
            split_extension("kestra:///a.b/c/x").is_err(),
            "a `.` in a directory: `/` in the extension"
        );
        assert_eq!(name_extension("kestra:///a.b/c/x"), ".tmp");
        assert_eq!(name_extension("kestra:///a/c/x.ion"), ".ion");
    }
}
