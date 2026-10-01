//! `kestra://` URIs (W9a spec §3): Kestra's layout, built and parsed here and
//! nowhere else.
//!
//! `kestra:///{ns-as-path}/{flow-slug}/executions/{exec}/{tail}`, the tail being
//! `inputs/{input}/{file}`, `tasks/{task-slug}/{taskRunId}/{file}`,
//! `trigger/{trigger-slug}/{file}` or a bare `{file}` (Kestra's `Loop` STORE
//! outputs). **A URI carries no workspace**: [`super::FileStore::object_uri`]
//! takes the workspace from its caller. `{exec}` is the WRITING execution -- a
//! `Loop` iteration's own id inside a `Loop` (the user's ruling, spec §3).

use md5::{Digest, Md5};
use uuid::Uuid;

/// Kestra's `StorageContext.KESTRA_PROTOCOL` plus the empty authority: every
/// URI egret accepts starts with exactly these ten bytes (migration 0161
/// checks the same with `left(uri, 10)`).
pub const SCHEME_PREFIX: &str = "kestra:///";

const EXECUTIONS: &str = "executions";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tail {
    Input {
        input: String,
        file: String,
    },
    Task {
        task_slug: String,
        task_run: String,
        file: String,
    },
    Trigger {
        trigger_slug: String,
        file: String,
    },
    /// `executions/{exec}/{file}`: Kestra's `Loop` STORE outputs (Task 1, `loop_outputs_store_uri`). Parsed and readable;
    /// egret writes it only if a task implements Loop `outputs` in STORE mode.
    Execution {
        file: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecUri {
    pub ns_path: String,
    pub flow_slug: String,
    pub execution: Uuid,
    pub tail: Tail,
}

/// Parse a `kestra://` execution-file URI.
///
/// Refuses, each by a reason naming what is wrong: a scheme other than
/// `kestra:///`; an empty segment (`//`, a trailing `/`); a `.` or `..`
/// segment; no `executions/{exec}` after at least a namespace and a flow; an
/// `{exec}` that is not a UUID in its canonical (lowercase, hyphenated)
/// spelling -- any other spelling would not round-trip and so would not match
/// the index row egret wrote; and a tail that is not one of the four layouts.
///
/// `{exec}` is the FIRST `executions` segment, at index 2 or later, followed by
/// a UUID. The parse is total over what egret writes, and
/// `parse(u)?.to_uri() == u` for every `u` it accepts, so the object key (built
/// from the URI's path) and the readability decision (made on `{exec}`) are
/// about the same string.
pub fn parse(uri: &str) -> Result<ExecUri, String> {
    let Some(path) = uri.strip_prefix(SCHEME_PREFIX) else {
        return Err(format!(
            "the scheme is not {SCHEME_PREFIX} (an execution file URI is \
             kestra:///<namespace>/<flow>/executions/<execution>/...)"
        ));
    };
    let segments: Vec<&str> = path.split('/').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err("it has an empty path segment".to_string());
    }
    if segments.iter().any(|s| *s == ".." || *s == ".") {
        return Err("it has a '.' or '..' path segment".to_string());
    }
    let mut saw_executions = false;
    let mut found = None;
    for (i, seg) in segments.iter().enumerate().skip(2) {
        if *seg != EXECUTIONS {
            continue;
        }
        saw_executions = true;
        if let Some(exec) = segments.get(i + 1).and_then(|s| canonical_uuid(s)) {
            found = Some((i, exec));
            break;
        }
    }
    let Some((i, execution)) = found else {
        return Err(if saw_executions {
            "the segment after 'executions' is not an execution id (a canonical UUID)".to_string()
        } else {
            "it is not an execution file: no <namespace>/<flow>/executions/<execution>/ \
             in its path"
                .to_string()
        });
    };
    let ns_path = segments[..i - 1].join("/");
    let flow_slug = segments[i - 1].to_string();
    let rest = &segments[i + 2..];
    let tail = match rest {
        [] => return Err("it names an execution, not a file under it".to_string()),
        ["inputs", input, file @ ..] if !file.is_empty() => Tail::Input {
            input: (*input).to_string(),
            file: file.join("/"),
        },
        ["tasks", task_slug, task_run, file @ ..] if !file.is_empty() => Tail::Task {
            task_slug: (*task_slug).to_string(),
            task_run: (*task_run).to_string(),
            file: file.join("/"),
        },
        ["trigger", trigger_slug, file @ ..] if !file.is_empty() => Tail::Trigger {
            trigger_slug: (*trigger_slug).to_string(),
            file: file.join("/"),
        },
        [first, ..] if matches!(*first, "inputs" | "tasks" | "trigger") => {
            return Err(format!("its '{first}' tail is incomplete"))
        }
        [file] => Tail::Execution {
            file: (*file).to_string(),
        },
        [first, ..] => {
            return Err(format!(
                "'{first}' is not an execution file tail (inputs, tasks or trigger)"
            ))
        }
    };
    Ok(ExecUri {
        ns_path,
        flow_slug,
        execution,
        tail,
    })
}

fn canonical_uuid(s: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(s).ok()?;
    (id.hyphenated().to_string() == s).then_some(id)
}

impl ExecUri {
    /// The URI's text. `parse(u)?.to_uri() == u` for every accepted `u`.
    pub fn to_uri(&self) -> String {
        let base = format!(
            "{SCHEME_PREFIX}{}/{}/{EXECUTIONS}/{}/",
            self.ns_path,
            self.flow_slug,
            self.execution.hyphenated()
        );
        match &self.tail {
            Tail::Input { input, file } => format!("{base}inputs/{input}/{file}"),
            Tail::Task {
                task_slug,
                task_run,
                file,
            } => format!("{base}tasks/{task_slug}/{task_run}/{file}"),
            Tail::Trigger { trigger_slug, file } => format!("{base}trigger/{trigger_slug}/{file}"),
            Tail::Execution { file } => format!("{base}{file}"),
        }
    }
}

/// `StorageContext.namespaceAsPath`: dots become slashes.
pub fn namespace_as_path(namespace: &str) -> String {
    namespace.replace('.', "/")
}

/// `Slugify.of` -- the slug Kestra puts into a URI for a flow, task or trigger
/// id. The body is `runners::filters::slugify`'s, shared rather than copied.
pub fn slugify_str(input: &str) -> String {
    crate::runners::slugify_str(input)
}

/// `IdUtils.from`: `FriendlyId.toFriendlyId(UUID.nameUUIDFromBytes(md5(s)))`,
/// i.e. md5 of the UTF-8 bytes, then `nameUUIDFromBytes` over those 16 bytes
/// (md5 again, version nibble 3, variant `10`), then base62 of the 128 bits
/// with no padding. `w9a-storage.json`'s `id_utils_from` rows decide.
pub fn id_utils_from(s: &str) -> String {
    let first = Md5::digest(s.as_bytes());
    let mut bytes: [u8; 16] = Md5::digest(first).into();
    bytes[6] = (bytes[6] & 0x0f) | 0x30;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    crate::runners::base62(u128::from_be_bytes(bytes), 0)
}

fn execution_base(namespace: &str, flow_id: &str, execution: Uuid) -> String {
    format!(
        "{SCHEME_PREFIX}{}/{}/{EXECUTIONS}/{}/",
        namespace_as_path(namespace),
        slugify_str(flow_id),
        execution.hyphenated()
    )
}

/// `execution` is the writing execution — the task run's own execution, an iteration's own id inside a `Loop`.
///
/// `kestra:///{ns}/{flow-slug}/executions/{exec}/tasks/{task-slug}/{taskRunId}/`
/// (`StorageContext.Task`); ends with `/`.
pub fn task_prefix(
    namespace: &str,
    flow_id: &str,
    execution: Uuid,
    task_id: &str,
    task_run: Uuid,
) -> String {
    format!(
        "{}tasks/{}/{}/",
        execution_base(namespace, flow_id, execution),
        slugify_str(task_id),
        task_run.hyphenated()
    )
}

/// `kestra:///{ns}/{flow-slug}/executions/{exec}/inputs/{input}/{file}`
/// (`StorageContext.Input`: the input's name and the file's are NOT slugified).
pub fn input_uri(
    namespace: &str,
    flow_id: &str,
    execution: Uuid,
    input: &str,
    file: &str,
) -> String {
    format!(
        "{}inputs/{input}/{file}",
        execution_base(namespace, flow_id, execution)
    )
}

/// `kestra:///{ns}/{flow-slug}/executions/{exec}/{file}` ([`Tail::Execution`]).
pub fn execution_file_uri(namespace: &str, flow_id: &str, execution: Uuid, file: &str) -> String {
    format!("{}{file}", execution_base(namespace, flow_id, execution))
}

/// `{task_prefix}{id_utils_from(abs_path)}-{basename with ' ' -> '+'}` (FilesService.java:94-97).
pub fn output_file_uri(task_prefix: &str, abs_path: &str) -> String {
    let basename = abs_path.rsplit('/').next().unwrap_or(abs_path);
    format!(
        "{task_prefix}{}-{}",
        id_utils_from(abs_path),
        basename.replace(' ', "+")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{Caps, FileStore};
    use serde_json::Value;
    use std::sync::Arc;

    fn oracle() -> Value {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/w9a-storage.json");
        serde_json::from_str(&std::fs::read_to_string(path).expect("read w9a-storage.json"))
            .expect("parse w9a-storage.json")
    }

    fn rows(key: &str) -> Vec<Value> {
        oracle()["egret_expected"][key]
            .as_array()
            .unwrap_or_else(|| panic!("egret_expected.{key} is an array"))
            .clone()
    }

    #[test]
    fn slugify_and_id_utils_match_the_artifact() {
        let slug = rows("slugify");
        let ids = rows("id_utils_from");
        // A zero beside a number that can disagree: the fixture's own counts.
        assert_eq!((slug.len(), ids.len()), (29, 19));
        for row in slug {
            let (input, want) = (
                row["input"].as_str().unwrap(),
                row["output"].as_str().unwrap(),
            );
            assert_eq!(slugify_str(input), want, "slugify({input:?})");
        }
        for row in ids {
            let (input, want) = (
                row["input"].as_str().unwrap(),
                row["output"].as_str().unwrap(),
            );
            assert_eq!(id_utils_from(input), want, "IdUtils.from({input:?})");
        }
    }

    /// Every `outputFiles` row is `output_file_uri` over the artifact's own task
    /// prefix; every `outputDirectory` row is that prefix plus the relative
    /// path, unhashed (Task 6 builds those; the prefix is this module's).
    /// The prefix is `task_prefix` with Kestra's base62 ids where egret's
    /// UUIDs go, which is the only difference between the two (spec §3).
    #[test]
    fn output_file_uri_matches_the_artifact() {
        let all = rows("output_file_uri");
        let mut by_source = (0, 0);
        for row in &all {
            let s = |k: &str| row[k].as_str().unwrap().to_string();
            let (exec, run) = (Uuid::from_u128(1), Uuid::from_u128(2));
            let prefix = task_prefix(&s("namespace"), &s("flow_id"), exec, &s("task_id"), run)
                .replace(&exec.to_string(), &s("execution_id"))
                .replace(&run.to_string(), &s("task_run_id"));
            let got = match s("source").as_str() {
                "outputFiles" => {
                    by_source.0 += 1;
                    output_file_uri(&prefix, &s("absolute_path"))
                }
                "outputDirectory" => {
                    by_source.1 += 1;
                    format!("{prefix}{}", s("rel_path"))
                }
                other => panic!("unknown source {other}"),
            };
            assert_eq!(got, s("uri"), "{row}");
        }
        assert_eq!(by_source, (4, 2), "every row was compared");
    }

    #[test]
    fn parse_refuses_by_name() {
        let id = "0e9a7b4c-3f53-4f0b-9b3e-8d1f2a6c7e10";
        let refused = [
            ("s3://b/k", "scheme"),
            ("file:///etc/passwd", "scheme"),
            ("nsfile:///a", "scheme"),
            ("kestra://ns/f/executions/x", "scheme"),
            (
                &*format!("kestra:///a/../b/executions/{id}/inputs/f/x"),
                "'..'",
            ),
            (
                &*format!("kestra:///ns/f/executions/{id}/tasks/t/r/../x"),
                "'..'",
            ),
            (
                "kestra:///ns/f/executions/not-a-uuid/inputs/f/x",
                "not an execution id",
            ),
            (
                &*format!("kestra:///ns/f/executions/{}/inputs/f/x", id.to_uppercase()),
                "not an execution id",
            ),
            (
                &*format!("kestra:///ns/f/executions/{id}/other/x"),
                "'other'",
            ),
            (
                &*format!("kestra:///ns//f/executions/{id}/inputs/f/x"),
                "empty path segment",
            ),
            (
                &*format!("kestra:///ns/f/executions/{id}/inputs/f/"),
                "empty path segment",
            ),
            (
                &*format!("kestra:///ns/f/executions/{id}/inputs/f"),
                "incomplete",
            ),
            (
                &*format!("kestra:///ns/f/executions/{id}"),
                "names an execution",
            ),
            (
                &*format!("kestra:///f/executions/{id}/x"),
                "not an execution file",
            ),
            (
                "kestra:///not-an-execution-file.txt",
                "not an execution file",
            ),
        ];
        for (uri, reason) in refused {
            let err = parse(uri).expect_err(uri);
            assert!(
                err.contains(reason),
                "{uri}: {err:?} should name {reason:?}"
            );
        }
        let accepted = parse(&format!("kestra:///ns/f/executions/{id}/123.ion")).unwrap();
        assert_eq!(
            accepted.tail,
            Tail::Execution {
                file: "123.ion".into()
            }
        );
        assert_eq!(accepted.execution.to_string(), id);
    }

    #[test]
    fn parse_to_uri_round_trips() {
        let (e, r) = (Uuid::new_v4(), Uuid::new_v4());
        let tp = task_prefix("w9a.storage", "Out_Files.Case-", e, "Shell_Task-", r);
        let uris = [
            input_uri("a.b.c", "My Flow", e, "file1", "data.csv"),
            output_file_uri(&tp, "/egret/working-dir/a b.txt"),
            format!("{tp}deep/d.txt"),
            format!("kestra:///w9a/f/executions/{e}/trigger/my-trigger/x.json"),
            execution_file_uri("ns", "f", e, "0.ion"),
            // A namespace segment named `executions` is still a namespace.
            format!("kestra:///a/executions/f/executions/{e}/inputs/i/f"),
        ];
        for u in uris {
            let p = parse(&u).unwrap_or_else(|err| panic!("{u}: {err}"));
            assert_eq!(p.execution, e, "{u}");
            assert_eq!(p.to_uri(), u);
        }
        let p = parse(&format!("{tp}f.txt")).unwrap();
        assert_eq!(
            p.tail,
            Tail::Task {
                task_slug: "shell-task".into(),
                task_run: r.to_string(),
                file: "f.txt".into()
            }
        );
        assert_eq!(
            (p.ns_path.as_str(), p.flow_slug.as_str()),
            ("w9a/storage", "out-filescase")
        );
        let p = parse(&format!(
            "kestra:///a/executions/f/executions/{e}/inputs/i/f"
        ))
        .unwrap();
        assert_eq!(
            (p.ns_path.as_str(), p.flow_slug.as_str()),
            ("a/executions", "f")
        );
    }

    fn store() -> FileStore {
        FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "bkt".into(),
            prefix: "workflow/".into(),
            caps: Caps::default(),
            retention_days: 30,
        }
    }

    #[test]
    fn the_object_key_takes_the_workspace_from_the_caller_and_nothing_from_the_uri() {
        let (caller, other, e) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let fs = store();
        // The URI's namespace path IS another workspace's id.
        let uri = format!("kestra:///{other}/f/executions/{e}/inputs/i/x.txt");
        let key = fs.object_uri(caller, &uri).unwrap();
        assert_eq!(
            key,
            format!("s3://bkt/workflow/{caller}/{other}/f/executions/{e}/inputs/i/x.txt")
        );
        assert!(key.starts_with(&format!("s3://bkt/workflow/{caller}/")));
        // A URI that does not parse has no key.
        let err = fs.object_uri(caller, "kestra:///a/../b").unwrap_err();
        assert!(err.contains("kestra:///a/../b"), "{err}");
        assert_eq!(
            fs.execution_prefix(caller, "a.b", "My_Flow", e),
            format!("s3://bkt/workflow/{caller}/a/b/my-flow/executions/{e}/")
        );
    }
}
