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

static SERIAL: Mutex<()> = Mutex::new(());

const MIB: usize = 1 << 20;

/// `read_values_capped(bytes, usize::MAX, cap)`, and the most heap it held
/// above what was live before it.
fn read_measured(bytes: Vec<u8>, cap: u64) -> (usize, Stop, usize) {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
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

/// The control: files egret and Kestra write read whole under the default
/// 10 MiB cap -- Kestra's `FileSerde` binary (a version marker and a symbol
/// table per row; the fixture's `DeduplicateItems` output, repeated to
/// 4 MiB) and `ionfmt` text (no symbol table) -- and their peak is what the
/// rows themselves hold, far under the file.
#[test]
fn kestra_binary_and_egret_text_files_read_whole() {
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
