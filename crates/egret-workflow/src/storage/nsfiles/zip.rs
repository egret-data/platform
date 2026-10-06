//! A streaming zip writer (W9b Task 9): `GET …/files/export`'s body.
//!
//! PKWARE `APPNOTE.TXT` 6.3.x, by section:
//!
//! - **STORED entries** (method 0, §4.4.5): nothing is compressed.
//! - **Two passes per entry, no data descriptor** (the user's ruling,
//!   2026-10-06, Task 9 fix round 1, I-2): the caller reads the body once
//!   through [`ZipWriter::checksum`] (its CRC-32, and its length held to the
//!   declared size), then [`ZipWriter::add`] writes a local header that
//!   carries the real CRC and sizes (flag bit 3 clear) and streams the body a
//!   second time, which must hold the same bytes (its length and CRC are
//!   checked again; a body that changed in between is an error). Java's
//!   `java.util.zip.ZipInputStream` -- the class Kestra expands an uploaded
//!   `.zip` with -- refuses a STORED entry with a data descriptor; this form it
//!   reads.
//! - **UTF-8 names** (flag bit 11, §4.4.4, appendix D).
//! - **ZIP64** (§4.3.14-§4.3.16, §4.5.3) only when a field overflows: an
//!   entry's size (the local header then carries the ZIP64 extra field with
//!   both sizes, the central header with both) or offset (the central
//!   header), the central directory's size or offset (32-bit fields, whose
//!   all-ones value is the ZIP64 sentinel and so never a value), or the entry
//!   count (16-bit).
//! - **The length is known before the first byte** ([`archive_len`]): every
//!   header's size follows from the names and the sizes alone, so the route
//!   sends `Content-Length` and a truncated body is detectable on every HTTP
//!   version (fix round 1, I-1).
//!
//! Memory: one copy buffer ([`COPY_BUF`]) for the writer's life, plus one
//! small [`CentralEntry`] per entry (its name and five numbers) until
//! [`ZipWriter::finish`] writes the central directory. The caller bounds the
//! entry count (the export lists at most `max_namespace_files` + 1 rows).
//!
//! The instruments are external readers (Python's `zipfile`, Info-ZIP's
//! `unzip -t`, Java's `ZipInputStream`), as W9a's hand-rolled ustar
//! (`storage/transfer.rs`) is checked by `tar`.

use chrono::{DateTime, Datelike, Timelike, Utc};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIR: u32 = 0x0605_4b50;
const ZIP64_END_OF_CENTRAL_DIR: u32 = 0x0606_4b50;
const ZIP64_END_LOCATOR: u32 = 0x0706_4b50;
/// §4.5.3: the ZIP64 extended information extra field's tag.
const ZIP64_EXTRA: u16 = 0x0001;
/// Bit 11 (UTF-8 names) only: bit 3 is clear, the local header carries the
/// CRC and the sizes.
const FLAGS: u16 = 1 << 11;
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

/// The fixed parts of each record (§4.3.7, §4.3.12, §4.3.14-§4.3.16).
const LOCAL_FIXED: u64 = 30;
const LOCAL_ZIP64_EXTRA: u64 = 4 + 16;
const CENTRAL_FIXED: u64 = 46;
const END_FIXED: u64 = 22;
const ZIP64_END_AND_LOCATOR: u64 = 56 + 20;

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

    /// The local header's length (not the body's) for a name of `n` bytes.
    fn local_len(&self, n: usize, size: u64) -> u64 {
        let extra = if self.fits32(size) {
            0
        } else {
            LOCAL_ZIP64_EXTRA
        };
        LOCAL_FIXED + n as u64 + extra
    }

    /// The central ZIP64 extra field's length: only the overflowing fields.
    fn central_extra_len(&self, size: u64, offset: u64) -> u64 {
        let (big_size, big_offset) = (!self.fits32(size), !self.fits32(offset));
        if big_size || big_offset {
            4 + 16 * u64::from(big_size) + 8 * u64::from(big_offset)
        } else {
            0
        }
    }

    fn central_len(&self, n: usize, size: u64, offset: u64) -> u64 {
        CENTRAL_FIXED + n as u64 + self.central_extra_len(size, offset)
    }

    fn needs_zip64_end(&self, count: u64, cd_size: u64, cd_offset: u64) -> bool {
        !self.fits16(count) || !self.fits32(cd_size) || !self.fits32(cd_offset)
    }
}

/// The exact length of the archive [`ZipWriter`] writes for `entries`
/// (each a name's length in bytes and a body's size), ZIP64 records
/// included: the export's `Content-Length`.
pub fn archive_len(entries: impl IntoIterator<Item = (usize, u64)>, limits: Limits) -> u64 {
    let (mut offset, mut cd_size, mut count) = (0u64, 0u64, 0u64);
    for (n, size) in entries {
        cd_size += limits.central_len(n, size, offset);
        offset += limits.local_len(n, size) + size;
        count += 1;
    }
    let end = if limits.needs_zip64_end(count, cd_size, offset) {
        ZIP64_END_AND_LOCATOR + END_FIXED
    } else {
        END_FIXED
    };
    offset + cd_size + end
}

/// One entry's central directory record, kept until [`ZipWriter::finish`].
struct CentralEntry {
    name: Box<str>,
    crc: u32,
    size: u64,
    offset: u64,
    time: u16,
    date: u16,
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

fn invalid_data(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}

/// The entry name rule, asserted here although every caller's name is an
/// `NsPath` already: relative (no leading `/`), no `.`, `..` or empty
/// segment, no backslash or NUL, no first segment that a Windows extractor
/// reads as a drive (`C:`, `c:x`; fix round 1, M-5), and it fits the 16-bit
/// length field. A name that would extract outside the target directory
/// never reaches an archive.
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
    let b = name.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return refuse(
            "it begins with a Windows drive ('C:'), which an extractor may treat as absolute",
        );
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

    /// Read `body` through, with the one copy buffer, writing nothing, and
    /// return its CRC-32: the first pass. A body shorter or longer than
    /// `size` is an `InvalidData` error naming the entry; at most `size + 1`
    /// bytes are taken from it.
    pub async fn checksum(
        &mut self,
        name: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        size: u64,
    ) -> std::io::Result<u32> {
        let (got, crc) = self.copy(body, size, false).await?;
        if got != size {
            return Err(invalid_data(format!(
                "zip entry {name:?}: its body holds {}, but its size is {size}",
                held(got, size)
            )));
        }
        Ok(crc)
    }

    /// The second pass: a local header carrying `crc` and `size` (from
    /// [`ZipWriter::checksum`]), then `body` streamed into the archive. A
    /// body that does not hold the same bytes (its length or its CRC differs)
    /// is an `InvalidData` error naming the entry, and the archive is then
    /// not finished: the caller must not call [`ZipWriter::finish`].
    pub async fn add(
        &mut self,
        name: &str,
        body: &mut (dyn AsyncRead + Send + Unpin),
        mtime: DateTime<Utc>,
        size: u64,
        crc: u32,
    ) -> std::io::Result<()> {
        check_name(name)?;
        let (time, date) = dos_time(mtime);
        let zip64 = !self.limits.fits32(size);
        let offset = self.offset;

        // §4.3.7: the local file header, with the CRC and the sizes; past the
        // 32-bit limit both sizes are in the ZIP64 extra field (§4.5.3: the
        // local header MUST carry both).
        let mut h = Le::default();
        h.u32(LOCAL_HEADER)
            .u16(if zip64 {
                VERSION_ZIP64
            } else {
                VERSION_DEFAULT
            })
            .u16(FLAGS)
            .u16(0)
            .u16(time)
            .u16(date)
            .u32(crc);
        let size32 = if zip64 { u32::MAX } else { size as u32 };
        h.u32(size32)
            .u32(size32)
            .u16(name.len() as u16)
            .u16(if zip64 { LOCAL_ZIP64_EXTRA as u16 } else { 0 })
            .bytes(name.as_bytes());
        if zip64 {
            h.u16(ZIP64_EXTRA).u16(16).u64(size).u64(size);
        }
        debug_assert_eq!(h.0.len() as u64, self.limits.local_len(name.len(), size));
        self.out.write_all(&h.0).await?;
        self.offset += h.0.len() as u64;

        let (got, again) = self.copy(body, size, true).await?;
        self.offset += got;
        if got != size || again != crc {
            return Err(invalid_data(format!(
                "zip entry {name:?} changed while it was exported: the second read held {} \
                 with CRC {again:08x}, the first {size} bytes with CRC {crc:08x}",
                held(got, size)
            )));
        }
        self.central.push(CentralEntry {
            name: name.into(),
            crc,
            size,
            offset,
            time,
            date,
        });
        Ok(())
    }

    /// Up to `size + 1` bytes of `body` through the CRC (and into `out`
    /// when `write`): the count and the CRC.
    async fn copy(
        &mut self,
        body: &mut (dyn AsyncRead + Send + Unpin),
        size: u64,
        write: bool,
    ) -> std::io::Result<(u64, u32)> {
        let mut crc = crc32fast::Hasher::new();
        let mut got: u64 = 0;
        // One byte past the size is the refusal.
        let budget = size.saturating_add(1);
        while got < budget {
            let room = self
                .buf
                .len()
                .min(usize::try_from(budget - got).unwrap_or(usize::MAX));
            let n = body.read(&mut self.buf[..room]).await?;
            if n == 0 {
                break;
            }
            crc.update(&self.buf[..n]);
            if write {
                self.out.write_all(&self.buf[..n]).await?;
            }
            got += n as u64;
        }
        Ok((got, crc.finalize()))
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
                let len = limits.central_extra_len(e.size, e.offset) - 4;
                extra.u16(ZIP64_EXTRA).u16(len as u16);
                if big_size {
                    extra.u64(e.size).u64(e.size);
                }
                if big_offset {
                    extra.u64(e.offset);
                }
            }
            let version = if big_size || big_offset {
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
        if limits.needs_zip64_end(count, cd_size, cd_offset) {
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

/// How many bytes a body held, said against its size: past it, only "more".
fn held(got: u64, size: u64) -> String {
    if got > size {
        format!("more than {size} bytes")
    } else {
        format!("{got} bytes")
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

    /// Is `bin` on PATH? Without it a reader's check is skipped and says so;
    /// with `CI` set it fails instead (fix round 1, M-2).
    fn have(bin: &str) -> bool {
        let found = std::process::Command::new(bin)
            .arg(match bin {
                "unzip" => "-v",
                "java" => "-version",
                _ => "--version",
            })
            .output()
            .is_ok_and(|o| o.status.success());
        if !found {
            assert!(
                std::env::var_os("CI").is_none(),
                "{bin} is not on PATH and CI is set: the archive's independent reader is missing"
            );
            eprintln!("SKIP {bin}: not found");
        }
        found
    }

    fn temp_zip(bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("w9b-t9-{}.zip", uuid::Uuid::new_v4()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// Python's own reading of the archive: `testzip()` (every CRC checked),
    /// then per entry its name, header CRC, `zlib.crc32` of the bytes read,
    /// size, central extra field, local header offset and the bytes as hex;
    /// then the ZIP64 end records as Python finds them from the end (no egret
    /// code involved).
    const PY_READ: &str = r#"
import json, struct, sys, zipfile, zlib
p = sys.argv[1]
z = zipfile.ZipFile(p)
bad = z.testzip()
assert bad is None, "testzip: bad entry %r" % bad
out = {"entries": []}
for i in z.infolist():
    data = z.read(i)
    assert i.flag_bits == 0x800, (i.filename, i.flag_bits)
    assert i.compress_type == zipfile.ZIP_STORED, i.compress_type
    out["entries"].append({"name": i.filename, "crc": i.CRC, "crc_read": zlib.crc32(data),
                           "size": i.file_size, "extra": i.extra.hex(),
                           "offset": i.header_offset,
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

    /// Python's reading, or `None` when there is no python3.
    fn python_reads(path: &std::path::Path) -> Option<serde_json::Value> {
        if !have("python3") {
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

    /// `unzip -tq`; nothing when there is no unzip.
    fn unzip_tests(path: &std::path::Path) {
        if !have("unzip") {
            return;
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
    }

    /// Java's `java.util.zip.ZipInputStream` (Kestra's import class, I-2)
    /// streams every entry from the front: name, bytes read, Java's CRC32 of
    /// them, and the bytes as hex when short.
    const JAVA_READ: &str = r#"
import java.io.*; import java.util.HexFormat; import java.util.zip.*;
public class W9bZipRead { public static void main(String[] a) throws Exception {
  try (ZipInputStream z = new ZipInputStream(new BufferedInputStream(new FileInputStream(a[0])))) {
    ZipEntry e; byte[] buf = new byte[65536];
    while ((e = z.getNextEntry()) != null) {
      CRC32 c = new CRC32(); ByteArrayOutputStream head = new ByteArrayOutputStream(); long n = 0; int r;
      while ((r = z.read(buf)) > 0) { c.update(buf, 0, r); n += r; if (head.size() <= 4096) head.write(buf, 0, r); }
      System.out.println(e.getName() + "\t" + n + "\t" + c.getValue() + "\t"
        + (n <= 4096 ? HexFormat.of().formatHex(head.toByteArray()) : "-"));
    }
  }
}}
"#;

    /// Java's reading as `(name, len, crc, bytes when short)`, or `None`
    /// when there is no java.
    /// One entry as Java streamed it: name, length, CRC, bytes when short.
    type JavaEntry = (String, u64, u32, Option<Vec<u8>>);

    fn java_reads(path: &std::path::Path) -> Option<Vec<JavaEntry>> {
        if !have("java") {
            return None;
        }
        let dir = std::env::temp_dir().join(format!("w9b-t9-java-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("W9bZipRead.java");
        std::fs::write(&src, JAVA_READ).unwrap();
        let o = std::process::Command::new("java")
            .arg("-Dstdout.encoding=UTF-8")
            .arg(&src)
            .arg(path)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            o.status.success(),
            "java.util.zip.ZipInputStream refused the archive: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        Some(
            String::from_utf8(o.stdout)
                .unwrap()
                .lines()
                .map(|l| {
                    let f: Vec<&str> = l.split('\t').collect();
                    let bytes = (f[3] != "-").then(|| hex::decode(f[3]).unwrap());
                    (
                        f[0].to_string(),
                        f[1].parse().unwrap(),
                        f[2].parse().unwrap(),
                        bytes,
                    )
                })
                .collect(),
        )
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

    /// Java streamed exactly `expected`: names, lengths, its own CRCs, and
    /// the bytes of the short ones.
    fn assert_java(read: &[JavaEntry], expected: &[(&str, Vec<u8>)]) {
        assert_eq!(read.len(), expected.len(), "java: entry count");
        for ((name, len, crc, bytes), (want_name, want)) in read.iter().zip(expected) {
            assert_eq!(name, want_name);
            assert_eq!(*len, want.len() as u64, "java: {name}");
            assert_eq!(*crc, crc32(want), "java: {name}: CRC");
            if let Some(b) = bytes {
                assert_eq!(b, want, "java: {name}: bytes");
            }
        }
    }

    /// The two passes over each entry, then `finish`; the archive's length
    /// is checked against [`archive_len`].
    async fn zip_of(entries: &[(&str, Vec<u8>)], limits: Limits) -> Vec<u8> {
        let mut w = ZipWriter::with_limits(Vec::new(), limits);
        for (name, bytes) in entries {
            let size = bytes.len() as u64;
            let mut r: &[u8] = bytes;
            let crc = w.checksum(name, &mut r, size).await.unwrap();
            let mut r: &[u8] = bytes;
            w.add(name, &mut r, t0(), size, crc).await.unwrap();
        }
        let out = w.finish().await.unwrap();
        let declared = archive_len(
            entries.iter().map(|(n, b)| (n.len(), b.len() as u64)),
            limits,
        );
        assert_eq!(out.len() as u64, declared, "archive_len");
        out
    }

    fn three() -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("empty file.txt", Vec::new()),
            ("é/one", vec![b'1']),
            ("sub/dir/three mib.bin", body_of(3 << 20)),
        ]
    }

    /// Every reader on one archive: Python (with the expected entries),
    /// unzip, Java's streaming reader. Returns Python's reading.
    fn read_by_all(bytes: &[u8], entries: &[(&str, Vec<u8>)]) -> Option<serde_json::Value> {
        let p = temp_zip(bytes);
        let read = python_reads(&p);
        unzip_tests(&p);
        let java = java_reads(&p);
        let _ = std::fs::remove_file(&p);
        if let Some(java) = java {
            assert_java(&java, entries);
        }
        if let Some(read) = &read {
            assert_entries(read, entries);
        }
        read
    }

    #[tokio::test]
    async fn python_zipfile_unzip_and_java_read_every_entry_byte_for_byte() {
        let entries = three();
        let bytes = zip_of(&entries, Limits::APPNOTE).await;
        if let Some(read) = read_by_all(&bytes, &entries) {
            assert!(read["zip64_end"].is_null(), "no field overflowed: no ZIP64");
            assert_eq!(read["eocd_count"], 3);
            assert!(read["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["extra"] == ""));
        }
        // The 3 MiB body's hex is not printed; its bytes, read back by
        // Python, by sha256.
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
        let names: Vec<String> = (0..N).map(|i| format!("e/{i:05}")).collect();
        let entries: Vec<(&str, Vec<u8>)> =
            names.iter().map(|n| (n.as_str(), Vec::new())).collect();
        let bytes = zip_of(&entries, Limits::APPNOTE).await;
        let Some(read) = read_by_all(&bytes, &entries) else {
            return;
        };
        assert_eq!(read["entries"][N - 1]["name"], "e/65535");
        assert_eq!(
            read["eocd_count"], 0xFFFF,
            "the 16-bit count is the sentinel"
        );
        assert_eq!(
            read["zip64_end"]["count"], N as u64,
            "the ZIP64 end record carries the count"
        );
    }

    /// The count's boundary (M-4), lowered to 3: two entries are a classic
    /// archive, three are ZIP64 (the sentinel is never a count). The real
    /// switch is `< 0xFFFF`, so 65 535 entries are ZIP64, as Go writes them.
    #[tokio::test]
    async fn the_entry_count_switches_to_zip64_at_its_limit() {
        let low = Limits {
            u32_limit: Limits::APPNOTE.u32_limit,
            u16_limit: 3,
        };
        let entries = vec![
            ("a", b"a".to_vec()),
            ("b", b"b".to_vec()),
            ("c", Vec::new()),
        ];
        let below = zip_of(&entries[..2], low).await;
        if let Some(read) = read_by_all(&below, &entries[..2]) {
            assert_eq!(read["eocd_count"], 2);
            assert!(read["zip64_end"].is_null());
        }
        let at = zip_of(&entries, low).await;
        if let Some(read) = read_by_all(&at, &entries) {
            assert_eq!(read["eocd_count"], 0xFFFF);
            assert_eq!(read["zip64_end"]["count"], 3);
        }
        assert!(!Limits::APPNOTE.fits16(65_535) && Limits::APPNOTE.fits16(65_534));
    }

    fn le64(v: u64) -> String {
        hex::encode(v.to_le_bytes())
    }

    /// Below the real limits nothing overflows, so the switch is lowered:
    /// sizes and offsets at or past 1000 bytes and counts past 2 go into
    /// ZIP64. Python, unzip and Java read it; each central ZIP64 extra field
    /// holds exactly the overflowing fields (M-4), and its length says so.
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
        let bytes = zip_of(&entries, low).await;
        let Some(read) = read_by_all(&bytes, &entries) else {
            return;
        };
        let e = |i: usize, k: &str| read["entries"][i][k].clone();
        let offset = |i: usize| e(i, "offset").as_u64().unwrap();
        assert_eq!(e(0, "extra"), "", "small, at offset 0");
        assert!(offset(1) < 1000, "{}", offset(1));
        assert_eq!(
            e(1, "extra"),
            format!("01001000{}{}", le64(2500), le64(2500)),
            "big/one: both sizes, no offset"
        );
        assert!(offset(2) >= 1000);
        assert_eq!(
            e(2, "extra"),
            format!("01000800{}", le64(offset(2))),
            "past/the/offset: the offset only"
        );
        assert_eq!(e(3, "extra"), format!("01000800{}", le64(offset(3))));
        assert_eq!(read["eocd_count"], 0xFFFF);
        assert_eq!(read["zip64_end"]["count"], 4);
        // The local header of big/one carries both sizes in its own extra.
        let lh = offset(1) as usize;
        assert_eq!(&bytes[lh + 18..lh + 26], &[0xFF; 8], "sentinel sizes");
        let n = "big/one".len();
        assert_eq!(
            hex::encode(&bytes[lh + 30 + n..lh + 30 + n + 20]),
            format!("01001000{}{}", le64(2500), le64(2500))
        );
    }

    #[test]
    fn archive_len_counts_every_record() {
        assert_eq!(archive_len([], Limits::APPNOTE), 22);
        // 30 + 1 + 3, then 46 + 1, then 22.
        assert_eq!(archive_len([(1, 3)], Limits::APPNOTE), 103);
        let low = Limits {
            u32_limit: 10,
            u16_limit: 2,
        };
        // Local 30 + 1 + 20 + 11; central 46 + 1 + 4 + 16; ZIP64 end 76 + 22.
        assert_eq!(archive_len([(1, 11)], low), 62 + 67 + 98);
    }

    /// The first pass holds the body to its size: a short body and a long one
    /// are each an `InvalidData` error naming the entry, and the long one is
    /// read no further than one byte past its size.
    #[tokio::test]
    async fn a_body_that_is_not_its_declared_size_is_an_error() {
        let mut w = ZipWriter::new(Vec::new());
        let mut short: &[u8] = b"four";
        let e = w.checksum("a/short", &mut short, 5).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            e.to_string().contains("a/short") && e.to_string().contains("4 bytes"),
            "{e}"
        );

        let long = body_of(10_000);
        let mut r: &[u8] = &long;
        let e = w.checksum("b/long", &mut r, 5).await.unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert!(e.to_string().contains("more than 5 bytes"), "{e}");
        assert_eq!(r.len(), 10_000 - 6, "read one byte past the size, no more");

        let mut exact: &[u8] = b"five!";
        assert_eq!(
            w.checksum("c", &mut exact, 5).await.unwrap(),
            crc32(b"five!")
        );
        assert_eq!(w.offset, 0, "the first pass writes nothing");
        assert!(w.out.is_empty());
    }

    /// The second pass must read what the first read: other bytes of the
    /// same length (another CRC), or another length, are an error naming the
    /// entry, and no central record is kept for it.
    #[tokio::test]
    async fn a_body_that_changed_between_the_passes_is_an_error() {
        for second in [&b"fivE!"[..], b"four", b"sixsix"] {
            let mut w = ZipWriter::new(Vec::new());
            let mut first: &[u8] = b"five!";
            let crc = w.checksum("x/y", &mut first, 5).await.unwrap();
            let mut r: &[u8] = second;
            let e = w.add("x/y", &mut r, t0(), 5, crc).await.unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::InvalidData, "{second:?}");
            assert!(
                e.to_string().contains("changed while it was exported"),
                "{e}"
            );
            assert!(w.central.is_empty());
        }
    }

    #[tokio::test]
    async fn an_entry_name_that_could_leave_the_target_is_refused() {
        for bad in [
            "", "/abs", "../up", "a/../b", "a/./b", "a//b", "a/", "a\\b", "nul\0", "C:/x", "c:x",
            "Z:",
        ] {
            let mut w = ZipWriter::new(Vec::new());
            let mut r: &[u8] = b"x";
            let e = w.add(bad, &mut r, t0(), 1, crc32(b"x")).await.unwrap_err();
            assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput, "{bad:?}");
            assert_eq!(w.offset, 0, "{bad:?}: nothing written");
        }
        for good in ["ab:c", "dir/C:", "1:x", "é/x y"] {
            check_name(good).unwrap();
        }
    }

    #[test]
    fn dos_time_is_the_utc_wall_clock_clamped_to_the_format() {
        let (t, d) = dos_time(t0());
        assert_eq!((t >> 11, (t >> 5) & 63, (t & 31) * 2), (12, 34, 56));
        assert_eq!((d >> 9) + 1980, 2026);
        assert_eq!(((d >> 5) & 15, d & 31), (10, 6));
        assert_eq!(dos_time("1970-01-01T00:00:00Z".parse().unwrap()), (0, 33));
        // Past 2107 (M-4): 2107-12-31 23:59:58, the format's last instant.
        let (t, d) = dos_time("2200-06-01T08:00:00Z".parse().unwrap());
        assert_eq!((t >> 11, (t >> 5) & 63, (t & 31) * 2), (23, 59, 58));
        assert_eq!(((d >> 9) + 1980, (d >> 5) & 15, d & 31), (2107, 12, 31));
        let (_, d) = dos_time("2107-03-04T00:00:00Z".parse().unwrap());
        assert_eq!(((d >> 9) + 1980, (d >> 5) & 15, d & 31), (2107, 3, 4));
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

    /// The second pass streams a 20 MiB body without holding it; the first
    /// pass writes nothing, so it is measured on its own counter.
    #[tokio::test]
    async fn the_writer_holds_no_entry_body_in_memory() {
        const SIZE: u64 = 20 << 20;
        let first = Arc::new(Flight::default());
        let flight = Arc::new(Flight::default());
        let mut w = ZipWriter::new(CountingSink(flight.clone()));
        let mut body = Generated {
            left: SIZE,
            flight: first.clone(),
        };
        let crc = w.checksum("big.bin", &mut body, SIZE).await.unwrap();
        assert_eq!(first.read.load(Ordering::SeqCst), SIZE);
        let mut body = Generated {
            left: SIZE,
            flight: flight.clone(),
        };
        w.add("big.bin", &mut body, t0(), SIZE, crc).await.unwrap();
        w.finish().await.unwrap();
        let (read, written, peak) = (
            flight.read.load(Ordering::SeqCst),
            flight.written.load(Ordering::SeqCst),
            flight.peak.load(Ordering::SeqCst),
        );
        // Control: the whole body went through.
        assert_eq!(read, SIZE);
        assert_eq!(written, archive_len([(7, SIZE)], Limits::APPNOTE));
        assert!(
            peak < 1 << 20,
            "the writer held {peak} bytes of the body at once; the bound is 1 MiB"
        );
    }
}
