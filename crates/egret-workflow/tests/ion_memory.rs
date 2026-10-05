//! W9a Task 12 fix round 3: what the one Ion reader (`storage::ion`) may
//! ALLOCATE for a file, measured by a counting global allocator in this test
//! binary -- not RSS, which is noisy and shared with the test harness. The
//! measure is the peak of live heap bytes above what was live when the read
//! started (the input is built before it), across every thread, so the tests
//! here take one lock and run one at a time.
//!
//! The reviewer's probes (ion-rs 1.0.1, release): 50 000 append-mode symbol
//! tables between values peaked at 3.05 GB RSS for a 203 MB file, and a
//! 10 MB text list of `0`s at 1.83 GB.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use egret_workflow::storage::ion::{read_values_capped, Stop};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::SeqCst) + layout.size();
            PEAK.fetch_max(now, Ordering::SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::SeqCst);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                let now = LIVE.fetch_add(new_size - layout.size(), Ordering::SeqCst) + new_size
                    - layout.size();
                PEAK.fetch_max(now, Ordering::SeqCst);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Held for a WHOLE test body (Task 13 fix round 1): the peak counts every
/// thread, so another test building its input (8 MB and more) while this one
/// measures was counted in this one's peak -- a flake in the parallel suite.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

const MIB: usize = 1 << 20;

/// `read_values_capped(bytes, usize::MAX, cap)`, and the most heap it held
/// above what was live before it.
fn read_measured(bytes: Vec<u8>, cap: u64) -> (usize, Stop, usize) {
    let base = LIVE.load(Ordering::SeqCst);
    PEAK.store(base, Ordering::SeqCst);
    let (rows, stop) = read_values_capped(std::io::Cursor::new(bytes), usize::MAX, cap);
    let rows = rows.len();
    let peak = PEAK.load(Ordering::SeqCst).saturating_sub(base);
    (rows, stop, peak)
}

/// `n` append-mode local symbol tables of `per` symbols `"a"`, each followed
/// by a value -- the reviewer's file, smaller.
fn interleaved_appends(n: usize, per: usize) -> Vec<u8> {
    let symbols = vec!["\"a\""; per].join(",");
    let mut text = String::new();
    for i in 0..n {
        text.push_str(&format!(
            "$ion_symbol_table::{{imports:$ion_symbol_table,symbols:[{symbols}]}} {i}\n"
        ));
    }
    text.into_bytes()
}

/// **(A)** Append-mode symbol tables interleaved with values: every value
/// resets the byte count, and ion-rs's symbol table grows by each table.
/// 2 000 × 1 000 symbols (8 MB) under a 1 MiB cap is refused naming the
/// cap, holding under 8 MiB at its peak (unbounded it held every symbol).
#[test]
fn interleaved_symbol_tables_are_refused_with_a_bounded_peak() {
    let _serial = serial();
    let file = interleaved_appends(2_000, 1_000);
    let size = file.len();
    let (rows, stop, peak) = read_measured(file, MIB as u64);
    eprintln!(
        "interleaved symbol tables: {size} bytes, {rows} values, peak {peak} bytes, {stop:?}"
    );
    let Stop::TooLarge(why) = stop else {
        panic!("not refused: {rows} values, peak {peak}, {stop:?}")
    };
    assert!(why.contains("max_read_bytes (1048576 bytes)"), "{why}");
    assert!(rows < 2_000, "{rows}");
    assert!(peak < 8 * MIB, "held {peak} bytes for a 1 MiB cap");
}

/// **(B)** One flat list of a million `0`s (2 MB text, 1 MB binary) under a
/// 4 MiB cap -- inside the byte cap -- is refused while it is scanned,
/// before the reader holds it whole, under the documented bound of about
/// 8 × the cap (`storage::ion::ELEMENT_BYTES`: `cap / 64` values at about
/// 270 bytes of the text reader's child cache and 256 of `Element`s each);
/// unbounded, the text list took 481 MB here.
#[test]
fn a_huge_flat_list_is_refused_before_it_is_built() {
    let _serial = serial();
    let n = 1_000_000;
    let mut text = String::with_capacity(2 * n + 2);
    text.push('[');
    text.push_str(&vec!["0"; n].join(","));
    text.push(']');
    let mut binary = vec![0xE0, 0x01, 0x00, 0xEA, 0xBE];
    // VarUInt length n, then n one-byte ints (0x20 is the int 0).
    let mut groups = Vec::new();
    let mut m = n;
    while m > 0 {
        groups.push((m & 0x7F) as u8);
        m >>= 7;
    }
    groups.reverse();
    *groups.last_mut().unwrap() |= 0x80;
    binary.extend(groups);
    binary.extend(std::iter::repeat_n(0x20u8, n));
    for (what, file) in [("text", text.into_bytes()), ("binary", binary)] {
        let (rows, stop, peak) = read_measured(file, 4 * MIB as u64);
        eprintln!("flat list {what}: {rows} values, peak {peak} bytes, {stop:?}");
        assert!(matches!(stop, Stop::TooLarge(_)), "{what}: {stop:?}");
        assert!(peak < 32 * MIB, "{what}: held {peak} bytes for a 4 MiB cap");
    }
}

/// A binary Ion header of type `t` (the high nibble) and `len` bytes, in
/// the long form: `tE` and a VarUInt length.
fn binary_header(t: u8, len: usize) -> Vec<u8> {
    let mut groups = Vec::new();
    let mut n = len;
    while n > 0 || groups.is_empty() {
        groups.push((n & 0x7F) as u8);
        n >>= 7;
    }
    groups.reverse();
    *groups.last_mut().unwrap() |= 0x80;
    let mut out = vec![(t << 4) | 0x0E];
    out.extend(groups);
    out
}

/// A binary append-mode local symbol table of `k` symbols with no text
/// (`null`, one byte each -- the cheapest symbol there is):
/// `$ion_symbol_table::{imports: $ion_symbol_table, symbols: [null, …]}`.
fn binary_null_symbol_table(k: usize) -> Vec<u8> {
    let mut list = binary_header(11, k);
    list.extend(std::iter::repeat_n(0x0Fu8, k));
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

const IVM: [u8; 4] = [0xE0, 0x01, 0x00, 0xEA];

/// **(A) fix round 4: back-to-back symbol tables.** The value count starts
/// again at each binary top level, so 63 append-mode tables of 163 000
/// `null` symbols each pass the per-value budget (163 840 at the 10 MiB
/// default), and the file (10 270 140 bytes) is under the byte cap. Charged
/// only at the next user value's yield, the reader had grown its table by
/// all 63 first: the reviewer measured a 494 MB peak. Charged after EVERY
/// item the reader consumes, the second table is refused with a peak of
/// about one table -- measured here against ONE such table, which reads.
#[test]
fn back_to_back_symbol_tables_are_refused_with_a_peak_of_one_table() {
    let _serial = serial();
    let cap = 10 * MIB as u64;
    let table = binary_null_symbol_table(163_000);
    let mut one = IVM.to_vec();
    one.extend(&table);
    one.extend([0x21, 0x01]);
    let (rows, stop, one_peak) = read_measured(one, cap);
    eprintln!("one symbol table: {rows} values, peak {one_peak} bytes, {stop:?}");
    assert!(
        matches!(stop, Stop::End),
        "the premise, one table: {stop:?}"
    );
    assert_eq!(rows, 1);

    let mut many = IVM.to_vec();
    for _ in 0..63 {
        many.extend(&table);
    }
    many.extend([0x21, 0x01]);
    assert_eq!(many.len(), 10_270_140, "the reviewer's file");
    let size = many.len();
    let (rows, stop, peak) = read_measured(many, cap);
    eprintln!("63 symbol tables: {size} bytes, {rows} values, peak {peak} bytes, {stop:?}");
    let Stop::TooLarge(why) = stop else {
        panic!("not refused: {rows} values, peak {peak}, {stop:?}")
    };
    assert!(why.contains("symbol"), "{why}");
    assert!(why.contains("max_read_bytes (10485760 bytes)"), "{why}");
    assert_eq!(rows, 0);
    assert!(
        peak <= 3 * one_peak,
        "held {peak} bytes for 63 tables where one table holds {one_peak}"
    );
}

/// **(B) fix round 4: element amplification in an s-expression.** Inside an
/// s-expression `ion-rs` splits a run of word characters at every
/// `$<digits>` symbol ID (`($1$2$3)` is three values), where the text scan
/// counted a run once: `(` + `$1` × 5 000 000 + `)` is 10 000 002 bytes,
/// under the 10 MiB cap, counted as about three values against a budget of
/// 163 840 -- and the reviewer measured `ion-rs` building 5 000 000 children
/// at a 1.92 GB peak (3.88 GB here, counted, in a debug build). Inside an
/// s-expression every word byte now counts, so it is refused while scanned
/// at the 163 841st: what the reader holds is the children it matched up
/// to there (its child cache, about 206 bytes each: 33.8 MB measured, under
/// the documented bound of about 8 × the cap).
#[test]
fn a_sexp_of_adjacent_symbol_ids_is_refused_before_it_is_split() {
    let _serial = serial();
    let n = 5_000_000;
    let mut text = String::with_capacity(2 * n + 2);
    text.push('(');
    for _ in 0..n {
        text.push_str("$1");
    }
    text.push(')');
    assert_eq!(text.len(), 10_000_002, "the reviewer's file");
    let (rows, stop, peak) = read_measured(text.into_bytes(), 10 * MIB as u64);
    eprintln!("sexp of symbol ids: {rows} values, peak {peak} bytes, {stop:?}");
    let Stop::TooLarge(why) = stop else {
        panic!("not refused: {rows} values, peak {peak}, {stop:?}")
    };
    assert!(why.contains("more than 163840 values"), "{why}");
    assert_eq!(rows, 0);
    assert!(peak < 8 * 10 * MIB, "held {peak} bytes for a 10 MiB cap");
}

/// **(B) fix round 5: a comment's bracket cannot hide the s-expression.**
/// The text scan counts an opening bracket inside a comment as a level
/// (nothing closes it: the scan is never behind `ion-rs`), and round 4 read
/// only the innermost level's s-expression bit -- so `( /*[*/ $1$2$3 )`,
/// `( //{` + newline + `$1$2$3 )` and `[( /*[*/ $1$2 )]`, which `ion-rs`
/// accepts, counted the run once again: five million `$1` built 5 000 000
/// children at about 2.27 GB (the re-review's probe). Now a word byte counts
/// while ANY open level is an s-expression, so each is refused while
/// scanned, with the round-4 peak.
#[test]
fn a_comment_bracket_cannot_hide_a_sexp_from_the_word_count() {
    let _serial = serial();
    let n = 5_000_000;
    let run = "$1".repeat(n);
    for (what, head, tail) in [
        ("block comment", "( /*[*/ ", " )"),
        ("line comment", "( //{\n ", " )"),
        ("nested in a list", "[( /*[*/ ", " )]"),
    ] {
        let text = format!("{head}{run}{tail}");
        let size = text.len();
        let (rows, stop, peak) = read_measured(text.into_bytes(), 10 * MIB as u64);
        eprintln!("{what}: {size} bytes, {rows} values, peak {peak} bytes, {stop:?}");
        let Stop::TooLarge(why) = stop else {
            panic!("{what}: not refused: {rows} values, peak {peak}, {stop:?}")
        };
        assert!(why.contains("more than 163840 values"), "{what}: {why}");
        assert_eq!(rows, 0, "{what}");
        assert!(
            peak < 8 * 10 * MIB,
            "{what}: held {peak} bytes for a 10 MiB cap"
        );
    }
}

/// The control: files egret and Kestra write read whole under the default
/// 10 MiB cap -- Kestra's `FileSerde` binary (a version marker and a symbol
/// table per row; the fixture's `DeduplicateItems` output, repeated to
/// 4 MiB) and `ionfmt` text (no symbol table) -- and their peak is what the
/// rows themselves hold, far under the file.
#[test]
fn kestra_binary_and_egret_text_files_read_whole() {
    let _serial = serial();
    use base64::Engine as _;
    let kestra = base64::engine::general_purpose::STANDARD
        .decode(
            "4AEA6umBg9aHtIFrgXbWioFiiyEC4AEA6umBg9aHtIFrgXbWioFhiyED4AEA6umBg9aHtIFrgXbWioFjiyEE",
        )
        .unwrap();
    let reps = (4 * MIB) / kestra.len();
    let binary = kestra.repeat(reps);
    let text = "{k:\"a\",v:1,s:\"some text\"}\n".repeat(4 * MIB / 26);
    let text_rows = 4 * MIB / 26;
    for (what, file, want) in [
        ("FileSerde binary", binary, 3 * reps),
        ("ionfmt text", text.into_bytes(), text_rows),
    ] {
        let (rows, stop, peak) = read_measured(file, 10 * MIB as u64);
        eprintln!("{what}: {rows} values, peak {peak} bytes, {stop:?}");
        assert!(matches!(stop, Stop::End), "{what}: {stop:?}");
        assert_eq!(rows, want, "{what}");
    }
}
