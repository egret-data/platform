//! The tar framing for moving files OUT of a task pod by exec — pure, in
//! memory, and hand-rolled. Files go IN through the streamed
//! [`write_tar`](crate::storage::transfer::write_tar) (W9a Task 5).
//!
//! # Why hand-rolled
//!
//! The only tar crate in this workspace's tree is `astral-tokio-tar`, pulled
//! in by `testcontainers` for tests; depending on it would put it in the
//! shipping binary for two functions over the simplest archive format there
//! is. Uploads are written as POSIX **ustar**; downloads read ustar plus the
//! two long-name extensions a `tar -c` may emit (GNU `L`, PAX `x`), which is
//! what busybox and GNU tar produce.
//!
//! # Why a download never touches egret's filesystem
//!
//! The Docker runner copies a declared file to a staging directory on the
//! worker host and must then refuse symlinks component by component, because a
//! link carried out of the container resolves against the HOST (commands.rs,
//! `SYMLINK_REFUSAL`). Here the archive is parsed in memory and only a
//! regular file's bytes are ever taken from it: a link entry has no content to
//! take, so there is nothing on the host to resolve. It is still refused by
//! name rather than returned as an empty file (what Kestra's `Files.copy` of a
//! link entry produces), because an empty output the author did not write is a
//! wrong answer that looks like a right one.

#[cfg(test)]
use std::collections::BTreeMap;

#[cfg(test)]
use crate::storage::transfer::header;
use crate::storage::transfer::BLOCK;

/// W5a's in-memory packer, kept ONLY as the reference
/// `storage::transfer::write_tar` is proven byte-identical to (W9a Task 5);
/// uploads stream through `write_tar`. This file goes in Task 6.
#[cfg(test)]
pub fn pack(files: &BTreeMap<String, String>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut dirs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for name in files.keys() {
        let mut acc = String::new();
        let parts: Vec<&str> = name.split('/').collect();
        for p in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(p);
            dirs.insert(format!("{acc}/"));
        }
    }
    for d in &dirs {
        out.extend_from_slice(&header(d, 0, b'5', 0o755)?);
    }
    for (name, contents) in files {
        let bytes = contents.as_bytes();
        out.extend_from_slice(&header(name, bytes.len() as u64, b'0', 0o644)?);
        out.extend_from_slice(bytes);
        out.resize(out.len().next_multiple_of(BLOCK), 0);
    }
    out.extend_from_slice(&[0u8; BLOCK * 2]);
    Ok(out)
}

/// What one declared output file turned out to be, read out of the archive a
/// `tar -c -C <working dir> -- <name>` produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Entry {
    /// A regular file, its bytes complete.
    File(Vec<u8>),
    /// A regular file over the per-file cap: its declared size. The contents
    /// were never read (the stream is cut at the cap).
    TooBig(u64),
    Directory,
    Symlink(String),
    /// A hard link, a device, a FIFO — anything that is not a file.
    Other(char),
}

/// The first entry of `archive` — the declared file itself.
///
/// `Ok(None)`: the archive holds no entry (tar found nothing to archive).
/// `Err`: the archive is corrupt or cut short, which is egret's transfer
/// failing, not the author's file.
pub fn unpack_one(archive: &[u8], max_file_bytes: usize) -> Result<Option<Entry>, String> {
    let mut pos = 0usize;
    let mut long_name: Option<String> = None;
    loop {
        let Some(h) = archive.get(pos..pos + BLOCK) else {
            return if pos == 0 || archive[pos.min(archive.len())..].iter().all(|b| *b == 0) {
                Ok(None)
            } else {
                Err("the archive ended inside a header".to_string())
            };
        };
        if h.iter().all(|b| *b == 0) {
            return Ok(None);
        }
        check_sum(h)?;
        let size = parse_size(&h[124..136])?;
        let kind = h[156];
        let data_start = pos + BLOCK;
        let data_len = usize::try_from(size).map_err(|_| "entry size overflows".to_string())?;
        match kind {
            b'L' | b'x' => {
                let data = archive
                    .get(data_start..data_start + data_len)
                    .ok_or("the archive ended inside a long-name entry")?;
                let name = if kind == b'L' {
                    Some(
                        String::from_utf8_lossy(data)
                            .trim_end_matches('\0')
                            .to_string(),
                    )
                } else {
                    pax_path(data)
                };
                if name.is_some() {
                    long_name = name;
                }
                pos = data_start + data_len.next_multiple_of(BLOCK);
                continue;
            }
            b'g' => {
                pos = data_start + data_len.next_multiple_of(BLOCK);
                continue;
            }
            _ => {}
        }
        let _name = long_name.take().unwrap_or_else(|| name_of(h));
        return Ok(Some(match kind {
            b'0' | 0 | b'7' => {
                if data_len > max_file_bytes {
                    Entry::TooBig(size)
                } else {
                    let data = archive
                        .get(data_start..data_start + data_len)
                        .ok_or("the archive ended inside a file's contents")?;
                    Entry::File(data.to_vec())
                }
            }
            b'5' => Entry::Directory,
            b'2' => Entry::Symlink(cstr(&h[157..257])),
            other => Entry::Other(other as char),
        }));
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn name_of(h: &[u8]) -> String {
    let base = cstr(&h[0..100]);
    let prefix = if &h[257..262] == b"ustar" {
        cstr(&h[345..500])
    } else {
        String::new()
    };
    if prefix.is_empty() {
        base
    } else {
        format!("{prefix}/{base}")
    }
}

/// PAX records are `"<len> <key>=<value>\n"`; the one that matters is `path`.
fn pax_path(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    text.lines().find_map(|l| {
        let (_, kv) = l.split_once(' ')?;
        kv.strip_prefix("path=").map(str::to_string)
    })
}

fn parse_size(f: &[u8]) -> Result<u64, String> {
    if f[0] & 0x80 != 0 {
        // GNU base-256: big-endian, first byte's top bit is the flag.
        let mut v: u64 = (f[0] & 0x7f) as u64;
        for b in &f[1..] {
            v = v
                .checked_mul(256)
                .and_then(|v| v.checked_add(*b as u64))
                .ok_or("entry size overflows")?;
        }
        return Ok(v);
    }
    let s = cstr(f);
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(s, 8).map_err(|_| format!("entry size {s:?} is not octal"))
}

fn check_sum(h: &[u8]) -> Result<(), String> {
    let stored = cstr(&h[148..156]);
    let stored = stored.trim();
    let want = u32::from_str_radix(stored, 8)
        .map_err(|_| format!("header checksum {stored:?} is not octal"))?;
    let got: u32 = h
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (148..156).contains(&i) {
                32
            } else {
                *b as u32
            }
        })
        .sum();
    if want != got {
        return Err(format!("header checksum {want} does not match {got}"));
    }
    Ok(())
}

/// What the author is told about one declared output, given what the archive
/// held — the Kubernetes counterpart of `commands::ingest_output_file`, with
/// its messages. `Ok(Some)` is the file's text; `Ok(None)` a declared file
/// the command did not leave, which is IGNORED as Kestra ignores it
/// (`w5a-scripts.json` `egret_expected.output_files.missing_declared_file`).
pub fn judge(
    rel: &str,
    entry: Option<Entry>,
    max_file_bytes: usize,
) -> Result<Option<Vec<u8>>, String> {
    match entry {
        None => Ok(None),
        Some(Entry::File(bytes)) => Ok(Some(bytes)),
        Some(Entry::TooBig(len)) => Err(format!(
            "the output file {rel:?} is {len} bytes, over the {max_file_bytes} an output may \
             carry. egret has no internal storage for a workflow yet (W9 owns it), so an output \
             file's CONTENTS are stored on the task run and re-rendered by every later task in \
             the execution; a prefix is not an answer"
        )),
        Some(Entry::Directory) => Err(format!(
            "`outputFiles` declares {rel:?}, which is a DIRECTORY. An output is JSON and egret \
             has no internal storage to hand back an archive URI for (W9 owns it), so name the \
             files inside it instead"
        )),
        Some(Entry::Symlink(target)) => Err(format!(
            "`outputFiles` declares {rel:?}, which is a SYMLINK (to {target:?}). A symlink is \
             refused rather than followed or returned empty: name the file itself instead"
        )),
        Some(Entry::Other(kind)) => Err(format!(
            "`outputFiles` declares {rel:?}, which is not a regular file (tar entry type \
             {kind:?}); name a regular file instead"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packed_archive_reads_back_file_by_file() {
        let files = BTreeMap::from([
            ("a.txt".to_string(), "hello\n".to_string()),
            ("dir/sub/b.txt".to_string(), "x".repeat(700)),
        ]);
        let tar = pack(&files).unwrap();
        assert_eq!(tar.len() % BLOCK, 0);
        // Parent directories come first, then the files, in name order.
        let first = unpack_one(&tar, 1 << 20).unwrap();
        assert_eq!(first, Some(Entry::Directory), "dir/ first");
        // The file entry for `a.txt` is after the two directory headers.
        let a = unpack_one(&tar[2 * BLOCK..], 1 << 20).unwrap();
        assert_eq!(a, Some(Entry::File(b"hello\n".to_vec())));
        let b = unpack_one(&tar[4 * BLOCK..], 1 << 20).unwrap();
        assert_eq!(b, Some(Entry::File(vec![b'x'; 700])));
        assert_eq!(
            unpack_one(&tar[4 * BLOCK..], 10).unwrap(),
            Some(Entry::TooBig(700))
        );
    }

    #[test]
    fn a_long_name_uses_the_ustar_prefix_and_an_impossible_one_is_refused() {
        let long = format!("{}/{}", "d".repeat(120), "f".repeat(90));
        let tar = pack(&BTreeMap::from([(long.clone(), "z".to_string())])).unwrap();
        let h = &tar[BLOCK..2 * BLOCK];
        assert_eq!(name_of(h), long);
        let err = pack(&BTreeMap::from([("f".repeat(101), String::new())])).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn an_empty_archive_is_no_entry_and_a_cut_one_is_an_error() {
        assert_eq!(unpack_one(&[], 10).unwrap(), None);
        assert_eq!(unpack_one(&[0u8; 1024], 10).unwrap(), None);
        let tar = pack(&BTreeMap::from([("a".to_string(), "abc".to_string())])).unwrap();
        assert!(unpack_one(&tar[..BLOCK + 1], 10).is_err());
        let mut bad = tar.clone();
        bad[0] ^= 1;
        assert!(unpack_one(&bad, 10).unwrap_err().contains("checksum"));
    }

    #[test]
    fn gnu_long_names_and_link_entries_are_read() {
        // A GNU `L` entry naming the next header, then a symlink.
        let mut tar = Vec::new();
        let name = "n".repeat(150);
        tar.extend_from_slice(
            &header("././@LongLink", name.len() as u64 + 1, b'L', 0o644).unwrap(),
        );
        let mut data = name.clone().into_bytes();
        data.push(0);
        data.resize(BLOCK, 0);
        tar.extend_from_slice(&data);
        let mut link = header("short", 0, b'2', 0o777).unwrap();
        link[157..168].copy_from_slice(b"/etc/passwd");
        // Recompute the checksum for the edited header.
        link[148..156].copy_from_slice(b"        ");
        let sum: u32 = link.iter().map(|b| *b as u32).sum();
        link[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        tar.extend_from_slice(&link);
        tar.extend_from_slice(&[0u8; 1024]);
        assert_eq!(
            unpack_one(&tar, 10).unwrap(),
            Some(Entry::Symlink("/etc/passwd".to_string()))
        );
    }

    #[test]
    fn every_non_file_is_judged_by_name() {
        assert_eq!(
            judge("x", None, 1).unwrap(),
            None,
            "a declared file the command did not leave is ignored, as Kestra"
        );
        assert!(judge("x", Some(Entry::Directory), 1)
            .unwrap_err()
            .contains("DIRECTORY"));
        assert!(judge("x", Some(Entry::Symlink("/e".into())), 1)
            .unwrap_err()
            .contains("SYMLINK"));
        assert!(judge("x", Some(Entry::TooBig(9)), 1)
            .unwrap_err()
            .contains("9 bytes"));
        assert!(judge("x", Some(Entry::Other('1')), 1)
            .unwrap_err()
            .contains("not a regular file"));
        assert_eq!(
            judge("x", Some(Entry::File(b"ok".to_vec())), 9).unwrap(),
            Some(b"ok".to_vec())
        );
    }
}
