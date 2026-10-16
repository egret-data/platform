//! The working-directory snapshot of a `WorkingDirectory` block (spec §4.3,
//! §5.2): an archive of the directory one child leaves behind, which the next
//! child starts from.
//!
//! [`pack`] and [`unpack`] are the pure half. They carry the three rules a
//! snapshot must keep whatever runner moves it: the bytes are capped, a
//! symbolic link is refused rather than followed, and no entry can name a
//! path outside the directory it is unpacked into.

use std::path::{Path, PathBuf};

use crate::storage::transfer::{header, EntryKind, TarReader};

/// Why a snapshot could not be taken or restored. Each variant is the child's
/// failure, and the block stops on it (spec §5.3).
#[derive(Debug, thiserror::Error)]
pub(crate) enum SnapshotError {
    /// The directory holds more than the cap. A snapshot is never truncated:
    /// a silently partial directory would make the next child wrong.
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

/// One thing [`pack`] writes: a directory, or a regular file with its bytes
/// still to be read.
enum Entry {
    Dir {
        name: String,
    },
    File {
        name: String,
        path: PathBuf,
        size: u64,
    },
}

/// The archive of everything under `dir`, parents before children, in name
/// order. The sizes are summed and checked against `cap` before a single byte
/// of file contents is read.
pub(crate) async fn pack(dir: &Path, cap: u64) -> Result<Vec<u8>, SnapshotError> {
    let mut entries = Vec::new();
    collect(dir, "", &mut entries)?;
    let total: u64 = entries
        .iter()
        .map(|entry| match entry {
            Entry::File { size, .. } => *size,
            Entry::Dir { .. } => 0,
        })
        .sum();
    if total > cap {
        return Err(SnapshotError::TooLarge { cap, size: total });
    }
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Dir { name } => {
                out.extend_from_slice(
                    &header(&name, 0, b'5', 0o755, 0).map_err(SnapshotError::Refused)?,
                );
            }
            Entry::File { name, path, .. } => {
                let bytes = tokio::fs::read(&path).await?;
                out.extend_from_slice(
                    &header(&name, bytes.len() as u64, b'0', 0o644, 0)
                        .map_err(SnapshotError::Refused)?,
                );
                out.extend_from_slice(&bytes);
                out.resize(out.len().next_multiple_of(512), 0);
            }
        }
    }
    out.extend_from_slice(&[0u8; 1024]);
    Ok(out)
}

/// Restore an archive written by [`pack`] under `dir`, which is created when
/// it does not exist. Only directories and regular files are written: any
/// other entry is refused before the file it would replace is touched.
pub(crate) async fn unpack(bytes: &[u8], dir: &Path) -> Result<(), SnapshotError> {
    let mut src = bytes;
    let mut reader = TarReader::new(&mut src);
    tokio::fs::create_dir_all(dir).await?;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|e| SnapshotError::Archive(e.to_string()))?
    {
        let relative = safe_relative(&entry.name)?;
        let path = dir.join(&relative);
        match entry.kind {
            EntryKind::Directory => tokio::fs::create_dir_all(&path).await?,
            EntryKind::File => {
                let mut data = Vec::new();
                tokio::io::AsyncReadExt::read_to_end(&mut reader.body(), &mut data).await?;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, &data).await?;
            }
            EntryKind::Symlink(_) | EntryKind::HardLink(_) | EntryKind::Other(_) => {
                return Err(SnapshotError::Refused(format!(
                    "the entry {:?}",
                    entry.name
                )));
            }
        }
    }
    Ok(())
}

/// Walk `root/rel`, appending its contents. A symbolic link is refused here,
/// by `symlink_metadata`, which is what keeps the walk from reading through it.
fn collect(root: &Path, rel: &str, out: &mut Vec<Entry>) -> Result<(), SnapshotError> {
    let mut children = std::fs::read_dir(root.join(rel))?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|child| child.file_name());
    for child in children {
        let base = child
            .file_name()
            .into_string()
            .map_err(|name| SnapshotError::Refused(format!("the non-UTF-8 name {name:?}")))?;
        let name = if rel.is_empty() {
            base
        } else {
            format!("{rel}/{base}")
        };
        let path = child.path();
        let kind = std::fs::symlink_metadata(&path)?.file_type();
        if kind.is_symlink() {
            return Err(SnapshotError::Refused(format!(
                "the symbolic link {name:?}"
            )));
        } else if kind.is_dir() {
            out.push(Entry::Dir {
                name: format!("{name}/"),
            });
            collect(root, &name, out)?;
        } else if kind.is_file() {
            let size = child.metadata()?.len();
            out.push(Entry::File { name, path, size });
        } else {
            return Err(SnapshotError::Refused(format!("the special file {name:?}")));
        }
    }
    Ok(())
}

/// An archive name as a path under the unpack directory. A name that is
/// absolute, empty in a component, or climbs with `..` is refused before any
/// file is created.
fn safe_relative(name: &str) -> Result<PathBuf, SnapshotError> {
    let trimmed = name.trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return Err(SnapshotError::Refused(format!(
            "the absolute name {name:?}"
        )));
    }
    let mut out = PathBuf::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => return Err(SnapshotError::Refused(format!("the name {name:?}"))),
            ".." => {
                return Err(SnapshotError::Refused(format!(
                    "the name {name:?}, which climbs out of the directory"
                )))
            }
            part => out.push(part),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("egret-snapshot-{tag}-{}", uuid::Uuid::new_v4()))
    }

    /// A directory with nested files, an empty directory, and binary bytes.
    fn sample_tree(root: &Path) {
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        std::fs::write(root.join("sub/b.bin"), [0u8, 255, 1, 2]).unwrap();
        std::fs::write(root.join("sub/deep/c.txt"), b"deep").unwrap();
    }

    /// An archive built by hand, so a test can hold entries [`pack`] would
    /// never write. `kind` is the ustar type flag.
    fn archive(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, kind, data) in entries {
            out.extend_from_slice(&header(name, data.len() as u64, *kind, 0o644, 0).unwrap());
            out.extend_from_slice(data);
            out.resize(out.len().next_multiple_of(512), 0);
        }
        out.extend_from_slice(&[0u8; 1024]);
        out
    }

    #[tokio::test]
    async fn a_directory_round_trips_through_pack_and_unpack() {
        let (src, dst) = (scratch("src"), scratch("dst"));
        sample_tree(&src);

        let bytes = pack(&src, 1 << 20).await.expect("packs");
        unpack(&bytes, &dst).await.expect("unpacks");

        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"hello");
        assert_eq!(
            std::fs::read(dst.join("sub/b.bin")).unwrap(),
            [0, 255, 1, 2]
        );
        assert_eq!(std::fs::read(dst.join("sub/deep/c.txt")).unwrap(), b"deep");
        assert!(dst.join("empty").is_dir(), "an empty directory survives");
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[tokio::test]
    async fn a_directory_over_the_cap_is_refused_and_names_the_cap() {
        let src = scratch("cap");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("big"), vec![7u8; 100]).unwrap();

        let err = pack(&src, 64)
            .await
            .expect_err("100 bytes is over a 64-byte cap");
        assert!(
            matches!(err, SnapshotError::TooLarge { cap: 64, size: 100 }),
            "{err:?}"
        );
        assert!(err.to_string().contains("64-byte cap"), "{err}");
        let _ = std::fs::remove_dir_all(&src);
    }

    #[tokio::test]
    async fn a_directory_exactly_at_the_cap_is_carried() {
        let src = scratch("exact");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("f"), vec![1u8; 64]).unwrap();

        pack(&src, 64).await.expect("64 bytes fits a 64-byte cap");
        let _ = std::fs::remove_dir_all(&src);
    }

    #[tokio::test]
    async fn a_symbolic_link_is_refused_and_its_target_is_not_read() {
        let (src, outside) = (scratch("link"), scratch("outside"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"not yours").unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), src.join("link")).unwrap();

        let err = pack(&src, 1 << 20)
            .await
            .expect_err("a symbolic link is not carried");
        assert!(
            matches!(&err, SnapshotError::Refused(m) if m.contains("link")),
            "{err:?}"
        );
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[tokio::test]
    async fn an_entry_that_climbs_out_of_the_directory_is_refused_before_anything_is_written() {
        let (base, dst) = (scratch("escape"), scratch("escape-dst"));
        std::fs::create_dir_all(&base).unwrap();
        let bytes = archive(&[("../escape.txt", b'0', b"x")]);

        let err = unpack(&bytes, &dst.join("inner"))
            .await
            .expect_err("`..` climbs out");
        assert!(matches!(err, SnapshotError::Refused(_)), "{err:?}");
        assert!(
            !dst.join("escape.txt").exists(),
            "nothing is written outside the directory"
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[tokio::test]
    async fn an_absolute_name_is_refused() {
        let dst = scratch("abs");
        let bytes = archive(&[("/etc/egret-snapshot-test", b'0', b"x")]);

        let err = unpack(&bytes, &dst).await.expect_err("an absolute name");
        assert!(matches!(err, SnapshotError::Refused(_)), "{err:?}");
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[tokio::test]
    async fn a_symbolic_link_entry_in_an_archive_is_refused() {
        let dst = scratch("linkentry");
        let bytes = archive(&[("ln", b'2', b"")]);

        let err = unpack(&bytes, &dst).await.expect_err("a link entry");
        assert!(matches!(err, SnapshotError::Refused(_)), "{err:?}");
        let _ = std::fs::remove_dir_all(&dst);
    }
}
