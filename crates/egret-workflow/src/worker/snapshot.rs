//! The working-directory snapshot of a `WorkingDirectory` block (spec §4.3,
//! §5.2): an archive of the directory one child leaves behind, which the next
//! child starts from.
//!
//! [`admit`] is the one function that carries a snapshot in either direction.
//! The directory lives inside a container (Docker, §4.1 option C), so the
//! worker never holds it: it reads a ustar stream from `docker cp c:<dir>/. -`
//! (copy-out) or from the object store (copy-in), checks every entry, and
//! writes the admitted archive onward, streamed. Nothing is held whole, so a
//! snapshot of the full `max_task_bytes` (5 GiB by default) costs a copy
//! buffer of memory, not a vector.
//!
//! The rules a snapshot keeps whatever moves it:
//!
//! - the content is capped, and an archive over the cap is refused before its
//!   bytes are written (never truncated: a partial directory would make the
//!   next child wrong);
//! - a symbolic link, a hard link or a device is refused, never followed;
//! - no entry may name a path outside the directory (`..`, an absolute name);
//! - the permission bits are kept, so an executable stays executable.

use std::path::PathBuf;

use crate::storage::transfer::{header, EntryKind, ReadError, TarReader};

/// Why a snapshot could not be taken or restored. Each variant is the child's
/// failure, and the block stops on it (spec §5.3).
#[derive(Debug, thiserror::Error)]
pub(crate) enum SnapshotError {
    /// The directory holds more than the cap. A snapshot is never truncated.
    #[error(
        "the working directory is {size} bytes, over the {cap}-byte cap (max_task_bytes), so \
         the block cannot carry it to the next task"
    )]
    TooLarge { cap: u64, size: u64 },
    /// Something a snapshot does not carry: a symbolic link, a hard link, a
    /// device, or a name that climbs out of the directory.
    #[error("a working-directory snapshot cannot carry {0}")]
    Refused(String),
    /// The archive does not read as ustar.
    #[error("the working-directory snapshot is corrupt: {0}")]
    Archive(String),
    #[error("the working-directory snapshot could not be read or written: {0}")]
    Io(#[from] std::io::Error),
}

impl From<ReadError> for SnapshotError {
    fn from(e: ReadError) -> Self {
        match e {
            ReadError::Io(io) => Self::Io(io),
            ReadError::Corrupt(m) => Self::Archive(m),
        }
    }
}

/// Stream the archive in `src` to `dst`, admitting only what a snapshot
/// carries. Returns the content bytes admitted.
///
/// The cap is checked against each entry's declared size **before** its
/// contents are copied, so an over-cap archive is refused after at most one
/// header has been written past the cap. The output is a fresh archive (the
/// root `./` entry is dropped, leading `./` is stripped, every header is
/// rewritten), so what is stored is exactly what was admitted.
pub(crate) async fn admit(
    src: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
    dst: &mut (dyn tokio::io::AsyncWrite + Send + Unpin),
    cap: u64,
) -> Result<u64, SnapshotError> {
    use tokio::io::{AsyncWriteExt, BufReader};
    let mtime = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut reader = BufReader::new(src);
    let mut tar = TarReader::new(&mut reader);
    let mut total: u64 = 0;
    while let Some(entry) = tar.next_entry().await? {
        let Some(rel) = normalise(&entry.name)? else {
            // The archive's own root (`./`, `.`): nothing to carry.
            continue;
        };
        match entry.kind {
            EntryKind::Directory => {
                let h = header(&format!("{rel}/"), 0, b'5', entry.mode, mtime)
                    .map_err(SnapshotError::Refused)?;
                dst.write_all(&h).await?;
            }
            EntryKind::File => {
                if total.saturating_add(entry.size) > cap {
                    return Err(SnapshotError::TooLarge {
                        cap,
                        size: total.saturating_add(entry.size),
                    });
                }
                total += entry.size;
                let h = header(&rel, entry.size, b'0', entry.mode, mtime)
                    .map_err(SnapshotError::Refused)?;
                dst.write_all(&h).await?;
                let mut body = tar.body();
                let copied = tokio::io::copy(&mut body, dst).await?;
                if copied != entry.size {
                    return Err(SnapshotError::Archive(format!(
                        "the entry {rel:?} ended after {copied} of {} bytes",
                        entry.size
                    )));
                }
                dst.write_all(&vec![0u8; padding(entry.size)]).await?;
            }
            EntryKind::Symlink(_) => {
                return Err(SnapshotError::Refused(format!("the symbolic link {rel:?}")))
            }
            EntryKind::HardLink(_) => {
                return Err(SnapshotError::Refused(format!("the hard link {rel:?}")))
            }
            EntryKind::Other(kind) => {
                return Err(SnapshotError::Refused(format!(
                    "the special entry {rel:?} (type {kind:?})"
                )))
            }
        }
    }
    dst.write_all(&[0u8; 1024]).await?;
    dst.flush().await?;
    Ok(total)
}

/// Zero bytes after a file's contents, to the next 512-byte block.
fn padding(size: u64) -> usize {
    ((512 - (size % 512)) % 512) as usize
}

/// An archive name as a relative path under the directory, with the leading
/// `./` of a `docker cp` archive removed. `None` is the archive's own root,
/// which carries nothing. A name that is absolute, has an empty component, or
/// climbs with `..` is refused.
fn normalise(name: &str) -> Result<Option<String>, SnapshotError> {
    let trimmed = name.trim_end_matches('/');
    if trimmed.starts_with('/') {
        return Err(SnapshotError::Refused(format!(
            "the absolute name {name:?}"
        )));
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "." => {}
            "" => {
                if trimmed.is_empty() {
                    // `./` or `.`: the root.
                    continue;
                }
                return Err(SnapshotError::Refused(format!("the name {name:?}")));
            }
            ".." => {
                return Err(SnapshotError::Refused(format!(
                    "the name {name:?}, which climbs out of the directory"
                )))
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    let path: PathBuf = parts.iter().collect();
    path.to_str()
        .map(|s| Some(s.replace('\\', "/")))
        .ok_or_else(|| SnapshotError::Refused(format!("the non-UTF-8 name {name:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An archive built by hand, so a test can hold entries a snapshot would
    /// never admit. `kind` is the ustar type flag.
    fn archive(entries: &[(&str, u8, u32, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, kind, mode, data) in entries {
            out.extend_from_slice(&header(name, data.len() as u64, *kind, *mode, 0).unwrap());
            out.extend_from_slice(data);
            out.resize(out.len().next_multiple_of(512), 0);
        }
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    async fn admit_bytes(bytes: &[u8], cap: u64) -> Result<(u64, Vec<u8>), SnapshotError> {
        let mut src = bytes;
        let mut dst = Vec::new();
        let n = admit(&mut src, &mut dst, cap).await?;
        Ok((n, dst))
    }

    /// The entries of an admitted archive, as `(name, kind, mode, bytes)`.
    async fn read_back(bytes: &[u8]) -> Vec<(String, EntryKind, u32, Vec<u8>)> {
        let mut src = bytes;
        let mut tar = TarReader::new(&mut src);
        let mut out = Vec::new();
        while let Some(entry) = tar.next_entry().await.expect("a valid archive") {
            let mut data = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut tar.body(), &mut data)
                .await
                .unwrap();
            out.push((entry.name, entry.kind, entry.mode, data));
        }
        out
    }

    #[tokio::test]
    async fn files_directories_and_their_modes_are_admitted_as_they_were() {
        let bytes = archive(&[
            ("./", b'5', 0o755, b""),
            ("./sub/", b'5', 0o750, b""),
            ("./a.txt", b'0', 0o644, b"hello"),
            ("./sub/run.sh", b'0', 0o755, b"#!/bin/sh\n"),
        ]);

        let (total, admitted) = admit_bytes(&bytes, 1 << 20).await.expect("admits");

        assert_eq!(total, 5 + 10);
        let got = read_back(&admitted).await;
        let names: Vec<&str> = got.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            names,
            ["sub/", "a.txt", "sub/run.sh"],
            "root dropped, ./ stripped"
        );
        assert_eq!(got[0].2, 0o750, "a directory keeps its mode");
        assert_eq!(got[2].2, 0o755, "the execute bit survives the copy");
        assert_eq!(got[1].3, b"hello");
    }

    #[tokio::test]
    async fn an_archive_over_the_cap_is_refused_and_names_the_cap() {
        let bytes = archive(&[("big", b'0', 0o644, &[7u8; 100])]);

        let err = admit_bytes(&bytes, 64).await.expect_err("100 > 64");

        assert!(
            matches!(err, SnapshotError::TooLarge { cap: 64, size: 100 }),
            "{err:?}"
        );
        assert!(err.to_string().contains("64-byte cap"), "{err}");
    }

    #[tokio::test]
    async fn an_archive_exactly_at_the_cap_is_admitted() {
        let bytes = archive(&[("f", b'0', 0o644, &[1u8; 64])]);

        let (total, _) = admit_bytes(&bytes, 64)
            .await
            .expect("64 fits a 64-byte cap");

        assert_eq!(total, 64);
    }

    #[tokio::test]
    async fn a_symbolic_link_entry_is_refused_not_followed() {
        let bytes = archive(&[("ln", b'2', 0o777, b"")]);

        let err = admit_bytes(&bytes, 1 << 20).await.expect_err("a link");

        assert!(
            matches!(&err, SnapshotError::Refused(m) if m.contains("symbolic link")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn a_hard_link_entry_is_refused() {
        let bytes = archive(&[("hl", b'1', 0o644, b"")]);

        let err = admit_bytes(&bytes, 1 << 20).await.expect_err("a hard link");

        assert!(matches!(err, SnapshotError::Refused(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_name_that_climbs_out_of_the_directory_is_refused() {
        let bytes = archive(&[("a/../../escape.txt", b'0', 0o644, b"x")]);

        let err = admit_bytes(&bytes, 1 << 20)
            .await
            .expect_err("`..` climbs out");

        assert!(matches!(err, SnapshotError::Refused(_)), "{err:?}");
    }

    #[tokio::test]
    async fn an_absolute_name_is_refused() {
        let bytes = archive(&[("/etc/egret-snapshot-test", b'0', 0o644, b"x")]);

        let err = admit_bytes(&bytes, 1 << 20).await.expect_err("absolute");

        assert!(matches!(err, SnapshotError::Refused(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_truncated_archive_is_an_error_not_a_short_snapshot() {
        let mut bytes = archive(&[("f", b'0', 0o644, &[9u8; 600])]);
        bytes.truncate(512 + 100);

        let err = admit_bytes(&bytes, 1 << 20).await.expect_err("cut short");

        assert!(
            matches!(err, SnapshotError::Archive(_) | SnapshotError::Io(_)),
            "{err:?}"
        );
    }
}
