//! A streaming zip writer (W9b Task 9): `GET …/files/export`'s body.
//!
//! PKWARE `APPNOTE.TXT` 6.3.x, by section:
//!
//! - **STORED entries** (method 0, §4.4.5): nothing is compressed, so nothing
//!   is buffered to be compressed.
//! - **A data descriptor after every entry** (general purpose flag bit 3,
//!   §4.3.9, §4.4.4): the local header carries a zero CRC and zero sizes, the
//!   body is copied once through a CRC-32 and a byte counter, and the CRC and
//!   sizes follow it. Nothing of a body is held to learn its size.
//! - **UTF-8 names** (flag bit 11, §4.4.4, appendix D).
//! - **ZIP64** (§4.3.14-§4.3.16, §4.5.3) only when a field overflows: an
//!   entry's size or offset, the central directory's size or offset (32-bit
//!   fields, whose all-ones value is the ZIP64 sentinel and so never a value),
//!   or the entry count (16-bit). An entry whose size is declared up front
//!   ([`ZipWriter::add_sized`]) at or past the 32-bit limit carries a ZIP64
//!   extra field in its local header too, so a reader that streams the
//!   archive from the front knows its descriptor holds 8-byte sizes (§4.3.9.2);
//!   one whose size is not declared ([`ZipWriter::add`]) and turns out past it
//!   gets the 8-byte descriptor and the central ZIP64 field only, as Go's
//!   `archive/zip` writes it.
//!
//! Memory: one copy buffer ([`COPY_BUF`]) for the writer's life, plus one
//! small [`CentralEntry`] per entry (its name and five numbers) until
//! [`ZipWriter::finish`] writes the central directory. The caller bounds the
//! entry count (the export lists at most `max_namespace_files` + 1 rows).
//!
//! The instruments are external readers (Python's `zipfile`, Info-ZIP's
//! `unzip -t`), as W9a's hand-rolled ustar (`storage/transfer.rs`) is
//! checked by `tar`.

use chrono::{DateTime, Datelike, Timelike, Utc};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const DATA_DESCRIPTOR: u32 = 0x0807_4b50;
const END_OF_CENTRAL_DIR: u32 = 0x0605_4b50;
const ZIP64_END_OF_CENTRAL_DIR: u32 = 0x0606_4b50;
const ZIP64_END_LOCATOR: u32 = 0x0706_4b50;
/// §4.5.3: the ZIP64 extended information extra field's tag.
const ZIP64_EXTRA: u16 = 0x0001;
/// Bit 3 (sizes and CRC in a data descriptor) and bit 11 (UTF-8 names).
const FLAGS: u16 = (1 << 3) | (1 << 11);
/// §4.4.3.2: 2.0 for a stored entry, 4.5 for one that uses ZIP64.
const VERSION_DEFAULT: u16 = 20;
const VERSION_ZIP64: u16 = 45;
/// §4.4.2: the upper byte of "version made by", 3 = UNIX, so the external
/// attributes below are a mode.
const MADE_BY_UNIX: u16 = 3 << 8;
/// A regular file, `rw-r--r--`, in the upper 16 bits (UNIX's convention).
const EXTERNAL_ATTRS: u32 = 0o100_644 << 16;
/// The one copy buffer.
pub const COPY_BUF: usize = 64 * 1024;

/// Where a field stops fitting. A 32-bit field's all-ones value is ZIP64's
/// sentinel, so a value at or past [`Limits::u32_limit`] goes into ZIP64; a
/// 16-bit count likewise at or past [`Limits::u16_limit`]. Production uses
/// [`Limits::APPNOTE`]; the tests lower them to exercise ZIP64 without
/// writing 4 GiB, and the archive stays valid either way: a reader takes the
/// value from the ZIP64 field whenever the 32-bit one is the sentinel.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub u32_limit: u64,
    pub u16_limit: u64,
}

impl Limits {
    pub const APPNOTE: Self = Self {
        u32_limit: 0xFFFF_FFFF,
        u16_limit: 0xFFFF,
    };

    fn fits32(&self, v: u64) -> bool {
        v < self.u32_limit
    }

    fn fits16(&self, v: u64) -> bool {
        v < self.u16_limit
    }
}

/// One entry's central directory record, kept until [`ZipWriter::finish`].
struct CentralEntry {
    name: Box<str>,
    crc: u32,
    size: u64,
    offset: u64,
    time: u16,
    date: u16,
    /// The local header carried a ZIP64 extra field.
    zip64_local: bool,
}

pub struct ZipWriter<W> {
    out: W,
    /// Bytes written so far: the next local header's offset.
    offset: u64,
    central: Vec<CentralEntry>,
    buf: Box<[u8]>,
    limits: Limits,
}

/// The bytes of a little-endian record.
#[derive(Default)]
struct Le(Vec<u8>);

impl Le {
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.0.extend_from_slice(v);
        self
    }
}

fn invalid_input(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
}

/// The entry name rule, asserted here although every caller's name is an
/// `NsPath` already: relative (no leading `/`), no `.`, `..` or empty
/// segment, no backslash or NUL, and it fits the 16-bit length field. A name
/// that would extract outside the target directory never reaches an archive.
pub fn check_name(name: &str) -> std::io::Result<()> {
    let refuse = |why: &str| Err(invalid_input(format!("zip entry name {name:?}: {why}")));
    if name.is_empty() {
        return refuse("it is empty");
    }
    if name.starts_with('/') {
        return refuse("it is absolute (a leading '/')");
    }
    if name.contains('\\') || name.contains('\0') {
        return refuse("it holds a backslash or a NUL");
    }
    if name
        .split('/')
        .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return refuse("it holds an empty, '.' or '..' segment");
    }
    if name.len() > usize::from(u16::MAX) {
        return refuse("it is longer than 65535 bytes");
    }
    Ok(())
}

/// §4.4.6: MS-DOS time and date, in UTC (the format has no zone); clamped to
/// the format's years, 1980-2107.
fn dos_time(t: DateTime<Utc>) -> (u16, u16) {
    let year = t.year();
    if year < 1980 {
        return (0, (1 << 5) | 1);
    }
    if year > 2107 {
        return ((23 << 11) | (59 << 5) | 29, (127 << 9) | (12 << 5) | 31);
    }
    let time = (t.hour() << 11) | (t.minute() << 5) | (t.second() / 2);
    let date = ((year as u32 - 1980) << 9) | (t.month() << 5) | t.day();
    (time as u16, date as u16)
}

impl<W: AsyncWrite + Send + Unpin> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        Self::with_limits(out, Limits::APPNOTE)
    }

    /// [`ZipWriter::new`] with lowered limits: the tests' ZIP64 switch.
    pub fn with_limits(out: W, limits: Limits) -> Self {
        Self {
            out,
            offset: 0,
            central: Vec::new(),
            buf: vec![0u8; COPY_BUF].into_boxed_slice(),
            limits,
        }
    }

    /// `name` is relative (no leading '/'); the reader is copied through a
    /// CRC32 + byte counter. Returns the entry's size.
    pub async fn add(
        &mut self,
        name: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        mtime: DateTime<Utc>,
    ) -> std::io::Result<u64> {
        self.entry(name, body, mtime, None).await
    }

    /// [`ZipWriter::add`] for a body whose size is known: a body that is
    /// shorter or longer than `size` is an error (`InvalidData`) naming the
    /// entry, and at most `size + 1` bytes are taken from it. The archive is
    /// then not finished: the caller must not call [`ZipWriter::finish`].
    pub async fn add_sized(
        &mut self,
        name: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        mtime: DateTime<Utc>,
        size: u64,
    ) -> std::io::Result<u64> {
        self.entry(name, body, mtime, Some(size)).await
    }

    async fn entry(
        &mut self,
        name: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        mtime: DateTime<Utc>,
        expected: Option<u64>,
    ) -> std::io::Result<u64> {
        check_name(name)?;
        let (time, date) = dos_time(mtime);
        let zip64_local = expected.is_some_and(|n| !self.limits.fits32(n));
        let offset = self.offset;

        // §4.3.7: the local file header.
        let mut h = Le::default();
        h.u32(LOCAL_HEADER)
            .u16(if zip64_local {
                VERSION_ZIP64
            } else {
                VERSION_DEFAULT
            })
            .u16(FLAGS)
            .u16(0)
            .u16(time)
            .u16(date)
            .u32(0);
        if zip64_local {
            // §4.3.9.1, §4.5.3: the sizes are in the extra field (zero here,
            // the data descriptor carries them).
            h.u32(u32::MAX).u32(u32::MAX);
        } else {
            h.u32(0).u32(0);
        }
        h.u16(name.len() as u16)
            .u16(if zip64_local { 20 } else { 0 })
            .bytes(name.as_bytes());
        if zip64_local {
            h.u16(ZIP64_EXTRA).u16(16).u64(0).u64(0);
        }
        self.out.write_all(&h.0).await?;
        self.offset += h.0.len() as u64;

        // The body, once, through the CRC and the counter.
        let mut crc = crc32fast::Hasher::new();
        let mut size: u64 = 0;
        // A declared size bounds the read at one byte past it: that byte is
        // the refusal.
        let budget = expected.map(|n| n.saturating_add(1));
        loop {
            let room = match budget {
                Some(b) => {
                    let left = b - size;
                    if left == 0 {
                        break;
                    }
                    self.buf
                        .len()
                        .min(usize::try_from(left).unwrap_or(usize::MAX))
                }
                None => self.buf.len(),
            };
            let n = body.read(&mut self.buf[..room]).await?;
            if n == 0 {
                break;
            }
            crc.update(&self.buf[..n]);
            self.out.write_all(&self.buf[..n]).await?;
            size += n as u64;
        }
        self.offset += size;
        if let Some(want) = expected {
            if size != want {
                let held = if size > want {
                    format!("more than {want} bytes")
                } else {
                    format!("{size} bytes")
                };
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("zip entry {name:?}: its body holds {held}, but its size is {want}"),
                ));
            }
        }
        let crc = crc.finalize();

        // §4.3.9: the data descriptor, 8-byte sizes when ZIP64 applies.
        let mut d = Le::default();
        d.u32(DATA_DESCRIPTOR).u32(crc);
        if zip64_local || !self.limits.fits32(size) {
            d.u64(size).u64(size);
        } else {
            d.u32(size as u32).u32(size as u32);
        }
        self.out.write_all(&d.0).await?;
        self.offset += d.0.len() as u64;

        self.central.push(CentralEntry {
            name: name.into(),
            crc,
            size,
            offset,
            time,
            date,
            zip64_local,
        });
        Ok(size)
    }

    /// The central directory and the end records; flushes and hands back
    /// the output (not shut down).
    pub async fn finish(mut self) -> std::io::Result<W> {
        let limits = self.limits;
        let cd_offset = self.offset;
        let mut cd_size: u64 = 0;
        for e in &self.central {
            let big_size = !limits.fits32(e.size);
            let big_offset = !limits.fits32(e.offset);
            // §4.5.3: only the overflowing fields, in this order.
            let mut extra = Le::default();
            if big_size || big_offset {
                let len = 8 * (2 * u16::from(big_size) + u16::from(big_offset));
                extra.u16(ZIP64_EXTRA).u16(len);
                if big_size {
                    extra.u64(e.size).u64(e.size);
                }
                if big_offset {
                    extra.u64(e.offset);
                }
            }
            let version = if e.zip64_local || big_size || big_offset {
                VERSION_ZIP64
            } else {
                VERSION_DEFAULT
            };
            let size32 = if big_size { u32::MAX } else { e.size as u32 };
            // §4.3.12: the central directory header.
            let mut c = Le::default();
            c.u32(CENTRAL_HEADER)
                .u16(MADE_BY_UNIX | version)
                .u16(version)
                .u16(FLAGS)
                .u16(0)
                .u16(e.time)
                .u16(e.date)
                .u32(e.crc)
                .u32(size32)
                .u32(size32)
                .u16(e.name.len() as u16)
                .u16(extra.0.len() as u16)
                .u16(0)
                .u16(0)
                .u16(0)
                .u32(EXTERNAL_ATTRS)
                .u32(if big_offset {
                    u32::MAX
                } else {
                    e.offset as u32
                })
                .bytes(e.name.as_bytes())
                .bytes(&extra.0);
            self.out.write_all(&c.0).await?;
            cd_size += c.0.len() as u64;
        }
        let count = self.central.len() as u64;
        let mut end = Le::default();
        let zip64 = !limits.fits16(count) || !limits.fits32(cd_size) || !limits.fits32(cd_offset);
        if zip64 {
            let record_offset = cd_offset + cd_size;
            // §4.3.14: the ZIP64 end of central directory record; its size
            // field counts what follows it (44 bytes, no extensible data).
            end.u32(ZIP64_END_OF_CENTRAL_DIR)
                .u64(44)
                .u16(MADE_BY_UNIX | VERSION_ZIP64)
                .u16(VERSION_ZIP64)
                .u32(0)
                .u32(0)
                .u64(count)
                .u64(count)
                .u64(cd_size)
                .u64(cd_offset);
            // §4.3.15: its locator.
            end.u32(ZIP64_END_LOCATOR).u32(0).u64(record_offset).u32(1);
        }
        // §4.3.16: the end of central directory record.
        let count16 = if limits.fits16(count) {
            count as u16
        } else {
            u16::MAX
        };
        let fit32 = |v: u64| {
            if limits.fits32(v) {
                v as u32
            } else {
                u32::MAX
            }
        };
        end.u32(END_OF_CENTRAL_DIR)
            .u16(0)
            .u16(0)
            .u16(count16)
            .u16(count16)
            .u32(fit32(cd_size))
            .u32(fit32(cd_offset))
            .u16(0);
        self.out.write_all(&end.0).await?;
        self.out.flush().await?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;

    fn t0() -> DateTime<Utc> {
        "2026-10-06T12:34:56Z".parse().unwrap()
    }

    /// A deterministic body of `len` bytes: `(i * 31 + 7) % 251`.
    fn body_of(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + 7) % 251) as u8).collect()
    }

    fn have(bin: &str) -> bool {
        std::process::Command::new(bin)
            .arg(if bin == "unzip" { "-v" } else { "--version" })
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn temp_zip(bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("w9b-t9-{}.zip", uuid::Uuid::new_v4()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// Python's own reading of the archive: `testzip()` (every CRC checked),
    /// then per entry its name, header CRC, `zlib.crc32` of the bytes read,
    /// size and the bytes as hex; then the ZIP64 end records as Python finds
    /// them from the end (no egret code involved).
    const PY_READ: &str = r#"
import json, struct, sys, zipfile, zlib
p = sys.argv[1]
z = zipfile.ZipFile(p)
bad = z.testzip()
assert bad is None, "testzip: bad entry %r" % bad
out = {"entries": []}
for i in z.infolist():
    data = z.read(i)
    assert i.flag_bits & 0x808 == 0x808, (i.filename, i.flag_bits)
    assert i.compress_type == zipfile.ZIP_STORED, i.compress_type
    out["entries"].append({"name": i.filename, "crc": i.CRC, "crc_read": zlib.crc32(data),
                           "size": i.file_size, "extra": i.extra.hex(),
                           "hex": data.hex() if len(data) <= 4096 else None,
                           "len": len(data)})
raw = open(p, "rb").read()
eocd = raw.rfind(b"PK\x05\x06")
_, _, _, n16, n16b, cd_size32, cd_off32, _ = struct.unpack("<IHHHHIIH", raw[eocd:eocd+22])
out["eocd_count"] = n16
out["zip64_end"] = None
loc = eocd - 20
if loc >= 0 and raw[loc:loc+4] == b"PK\x06\x07":
    _, _, rec, _ = struct.unpack("<IIQI", raw[loc:loc+20])
    sig, size, _, _, _, _, n, total, cd_size, cd_off = struct.unpack("<IQHHIIQQQQ", raw[rec:rec+56])
    assert sig == 0x06064b50, hex(sig)
    out["zip64_end"] = {"count": total, "cd_size": cd_size, "cd_offset": cd_off}
print(json.dumps(out))
"#;

    /// Python's reading, or `None` (printed) when there is no python3.
    fn python_reads(path: &std::path::Path) -> Option<serde_json::Value> {
        if !have("python3") {
            eprintln!("SKIP python3: not found");
            return None;
        }
        let o = std::process::Command::new("python3")
            .arg("-c")
            .arg(PY_READ)
            .arg(path)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "python's zipfile refused the archive: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        Some(serde_json::from_slice(&o.stdout).unwrap())
    }

    /// `unzip -tq`, or `false` (printed) when there is no unzip.
    fn unzip_tests(path: &std::path::Path) -> bool {
        if !have("unzip") {
            eprintln!("SKIP unzip: not found");
            return false;
        }
        let o = std::process::Command::new("unzip")
            .arg("-tq")
            .arg(path)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&o.stdout).into_owned();
        assert!(
            o.status.success() && said.contains("No errors detected"),
            "unzip -t refused the archive: {said}{}",
            String::from_utf8_lossy(&o.stderr)
        );
        true
    }

    /// The bitwise CRC-32 (IEEE), not crc32fast: the expected values.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// Every entry Python read is `expected`'s, in order: name, size, bytes,
    /// and both CRCs (the header's and Python's own over the bytes) equal to
    /// the bitwise CRC of the expected bytes.
    fn assert_entries(read: &serde_json::Value, expected: &[(&str, Vec<u8>)]) {
        let got = read["entries"].as_array().unwrap();
        assert_eq!(got.len(), expected.len(), "entry count");
        for (g, (name, bytes)) in got.iter().zip(expected) {
            assert_eq!(g["name"], *name);
            assert_eq!(g["size"], bytes.len() as u64, "{name}");
            assert_eq!(g["len"], bytes.len() as u64, "{name}");
            let want = u64::from(crc32(bytes));
            assert_eq!(g["crc"], want, "{name}: header CRC");
            assert_eq!(g["crc_read"], want, "{name}: CRC of the bytes read");
            if let Some(h) = g["hex"].as_str() {
                assert_eq!(hex::decode(h).unwrap(), *bytes, "{name}: bytes");
            }
        }
    }

    async fn zip_of(entries: &[(&str, Vec<u8>)], limits: Limits, sized: bool) -> Vec<u8> {
        let mut w = ZipWriter::with_limits(Vec::new(), limits);
        for (name, bytes) in entries {
            let mut r: &[u8] = bytes;
            let n = if sized {
                w.add_sized(name, &mut r, t0(), bytes.len() as u64).await
            } else {
                w.add(name, &mut r, t0()).await
            }
            .unwrap();
            assert_eq!(n, bytes.len() as u64);
        }
        w.finish().await.unwrap()
    }

    fn three() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("empty file.txt", Vec::new()),
            ("é/one", vec![b'1']),
            ("sub/dir/three mib.bin", body_of(3 << 20)),
        ]
    }

    #[tokio::test]
    async fn python_zipfile_and_unzip_read_every_entry_byte_for_byte() {
        let entries = three();
        let bytes = zip_of(&entries, Limits::APPNOTE, false).await;
        let p = temp_zip(&bytes);
        let read = python_reads(&p);
        let tested = unzip_tests(&p);
        let _ = std::fs::remove_file(&p);
        if let Some(read) = read {
            assert_entries(&read, &entries);
            assert!(read["zip64_end"].is_null(), "no field overflowed: no ZIP64");
            assert_eq!(read["eocd_count"], 3);
        }
        let _ = tested;
        // The 3 MiB body's hex is not printed; its bytes are pinned by both
        // CRCs over exactly its length. Its last byte, read back by Python:
        if have("python3") {
            let p = temp_zip(&bytes);
            let o = std::process::Command::new("python3")
                .arg("-c")
                .arg(
                    "import sys,zipfile,hashlib;\
                     print(hashlib.sha256(zipfile.ZipFile(sys.argv[1]).read('sub/dir/three mib.bin')).hexdigest())",
                )
                .arg(&p)
                .output()
                .unwrap();
            let _ = std::fs::remove_file(&p);
            use sha2::Digest as _;
            let want = hex::encode(sha2::Sha256::digest(&entries[2].1));
            assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), want);
        }
    }

    #[tokio::test]
    async fn a_zip64_archive_is_written_when_the_entry_count_passes_65535() {
        const N: usize = 65_536;
        let mut w = ZipWriter::new(Vec::new());
        for i in 0..N {
            let mut r: &[u8] = &[];
            w.add(&format!("e/{i:05}"), &mut r, t0()).await.unwrap();
        }
        let bytes = w.finish().await.unwrap();
        let p = temp_zip(&bytes);
        let read = python_reads(&p);
        unzip_tests(&p);
        let _ = std::fs::remove_file(&p);
        let Some(read) = read else { return };
        let got = read["entries"].as_array().unwrap();
        assert_eq!(got.len(), N);
        assert_eq!(got[N - 1]["name"], "e/65535");
        assert_eq!(
            read["eocd_count"], 0xFFFF,
            "the 16-bit count is the sentinel"
        );
        assert_eq!(
            read["zip64_end"]["count"], N as u64,
            "the ZIP64 end record carries the count"
        );
    }

    /// Below the real limits nothing overflows, so the switch is lowered:
    /// sizes and offsets past 1000 bytes and counts past 2 go into ZIP64,
    /// both for a declared size (a ZIP64 local header) and for an undeclared
    /// one (an 8-byte descriptor only). Python and unzip read both.
    #[tokio::test]
    async fn every_zip64_field_is_read_back_when_the_limits_are_lowered() {
        let low = Limits {
            u32_limit: 1000,
            u16_limit: 3,
        };
        let entries = vec![
            ("small", body_of(10)),
            ("big/one", body_of(2500)),
            ("past/the/offset", body_of(700)),
            ("last", Vec::new()),
        ];
        for sized in [false, true] {
            let bytes = zip_of(&entries, low, sized).await;
            let p = temp_zip(&bytes);
            let read = python_reads(&p);
            unzip_tests(&p);
            let _ = std::fs::remove_file(&p);
            let Some(read) = read else { return };
            assert_entries(&read, &entries);
            assert_eq!(read["eocd_count"], 0xFFFF, "sized={sized}");
            assert_eq!(read["zip64_end"]["count"], 4, "sized={sized}");
            // The central ZIP64 extra field (tag 0x0001) is where a reader
            // takes each overflowing value from: present for the entry past
            // the size limit and for the one past the offset limit, absent
            // for the one that fits both.
            let extra = |i: usize| read["entries"][i]["extra"].as_str().unwrap().to_string();
            assert_eq!(extra(0), "", "sized={sized}: small, at offset 0");
            assert!(extra(1).starts_with("0100"), "sized={sized}: {}", extra(1));
            assert!(extra(2).starts_with("0100"), "sized={sized}: {}", extra(2));
            let cd_offset = read["zip64_end"]["cd_offset"].as_u64().unwrap();
            assert!(cd_offset > 3200, "sized={sized}: {cd_offset}");
        }
    }

    /// A declared size is enforced: a short body and a long one are each an
    /// `InvalidData` error naming the entry, and the long one is read no
    /// further than one byte past its size.
    #[tokio::test]
    async fn a_body_that_is_not_its_declared_size_is_an_error() {
        let mut w = ZipWriter::new(Vec::new());
        let mut short: &[u8] = b"four";
        let e = w
            .add_sized("a/short", &mut short, t0(), 5)
            .await
            .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            e.to_string().contains("a/short") && e.to_string().contains("4 bytes"),
            "{e}"
        );

        let mut w = ZipWriter::new(Vec::new());
        let long = body_of(10_000);
        let mut r: &[u8] = &long;
        let e = w.add_sized("b/long", &mut r, t0(), 5).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("more than 5 bytes"), "{e}");
        assert_eq!(r.len(), 10_000 - 6, "read one byte past the size, no more");

        let mut w = ZipWriter::new(Vec::new());
        let mut exact: &[u8] = b"five!";
        assert_eq!(w.add_sized("c", &mut exact, t0(), 5).await.unwrap(), 5);
    }

    #[tokio::test]
    async fn an_entry_name_that_could_leave_the_target_is_refused() {
        for bad in [
            "", "/abs", "../up", "a/../b", "a/./b", "a//b", "a/", "a\\b", "nul\0",
        ] {
            let mut w = ZipWriter::new(Vec::new());
            let mut r: &[u8] = b"x";
            let e = w.add(bad, &mut r, t0()).await.unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
            assert_eq!(w.offset, 0, "{bad:?}: nothing written");
        }
    }

    #[test]
    fn dos_time_is_the_utc_wall_clock_clamped_to_the_format() {
        let (t, d) = dos_time(t0());
        assert_eq!((t >> 11, (t >> 5) & 63, (t & 31) * 2), (12, 34, 56));
        assert_eq!((d >> 9) + 1980, 2026);
        assert_eq!(((d >> 5) & 15, d & 31), (10, 6));
        assert_eq!(dos_time("1970-01-01T00:00:00Z".parse().unwrap()), (0, 33));
    }

    /// Bytes produced by the body and not yet written to the sink, at their
    /// highest: what the writer held of a body at once (W9a's measure,
    /// `storage/transfer.rs`).
    #[derive(Default)]
    struct Flight {
        read: AtomicU64,
        written: AtomicU64,
        peak: AtomicU64,
    }

    impl Flight {
        fn sample(&self) {
            let held = self
                .read
                .load(Ordering::SeqCst)
                .saturating_sub(self.written.load(Ordering::SeqCst));
            self.peak.fetch_max(held, Ordering::SeqCst);
        }
    }

    /// `len` generated bytes, counted as they are handed out.
    struct Generated {
        left: u64,
        flight: Arc<Flight>,
    }

    impl AsyncRead for Generated {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let n = (buf.remaining() as u64).min(self.left) as usize;
            buf.put_slice(&vec![0x5a; n]);
            self.left -= n as u64;
            self.flight.read.fetch_add(n as u64, Ordering::SeqCst);
            self.flight.sample();
            Poll::Ready(Ok(()))
        }
    }

    struct CountingSink(Arc<Flight>);

    impl AsyncWrite for CountingSink {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.0.sample();
            self.0.written.fetch_add(buf.len() as u64, Ordering::SeqCst);
            Poll::Ready(Ok(buf.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn the_writer_holds_no_entry_body_in_memory() {
        const SIZE: u64 = 20 << 20;
        let flight = Arc::new(Flight::default());
        let mut w = ZipWriter::new(CountingSink(flight.clone()));
        let mut body = Generated {
            left: SIZE,
            flight: flight.clone(),
        };
        assert_eq!(w.add("big.bin", &mut body, t0()).await.unwrap(), SIZE);
        w.finish().await.unwrap();
        let (read, written, peak) = (
            flight.read.load(Ordering::SeqCst),
            flight.written.load(Ordering::SeqCst),
            flight.peak.load(Ordering::SeqCst),
        );
        // Control: the whole body went through.
        assert_eq!(read, SIZE);
        assert!(written > SIZE, "{written}");
        assert!(
            peak < 1 << 20,
            "the writer held {peak} bytes of the body at once; the bound is 1 MiB"
        );
    }
}
