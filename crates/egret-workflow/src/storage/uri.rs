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
/// URI egret accepts starts with exactly these ten bytes (migration 0166
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
/// Also refused: `\`, `?`, `#` and control characters anywhere in the path.
///
/// `{exec}` is the FIRST `executions` segment, at index 2 or later, followed by
/// a UUID, and `parse(u)?.to_uri() == u` for every `u` it accepts, so the
/// object key (built from the URI's path) and the readability decision (made
/// on `{exec}`) are about the same string. **The parse is NOT total over every
/// namespace publish accepts** (`a..b`, `a.`, or one holding
/// `executions.<uuid>`): egret's builders read each URI back through this
/// function ([`checked`]) and refuse one it would refuse or misread, as the
/// author's error.
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
    // Defence in depth before a pod's `outputDirectory` paths are indexed
    // (Task 6): `\` (a separator elsewhere), `?` and `#` (a URI's query and
    // fragment) and control characters never belong to a file name egret
    // writes. `%` stays legal: Kestra keeps an uploaded name verbatim and the
    // artifact measured `inputs/f/a%20b.txt` (`w9a-storage.json`
    // `file_input.filenames.percent20`). Nothing in egret percent-decodes a
    // URI, so `%2e%2e` is a literal segment, never `..`.
    if let Some(c) = path
        .chars()
        .find(|c| matches!(c, '\\' | '?' | '#') || c.is_control())
    {
        return Err(format!(
            "it holds the character {c:?}, which no execution file URI may"
        ));
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
///
/// `Err` is the AUTHOR's error (see [`checked`]): the namespace, flow or task
/// id gives a URI egret's own [`parse`] would refuse or misread. The task
/// fails naming it; it is never `Error::Internal`, which inside the commit
/// transaction would fence the run and re-claim it for ever.
pub fn task_prefix(
    namespace: &str,
    flow_id: &str,
    execution: Uuid,
    task_id: &str,
    task_run: Uuid,
) -> Result<String, String> {
    let prefix = format!(
        "{}tasks/{}/{}/",
        execution_base(namespace, flow_id, execution),
        slugify_str(task_id),
        task_run.hyphenated()
    );
    // A prefix is not a file URI (it ends with `/`); the check reads back a
    // file under it.
    let probe = format!("{prefix}{PROBE_FILE}");
    checked(
        &probe,
        execution,
        &Tail::Task {
            task_slug: slugify_str(task_id),
            task_run: task_run.hyphenated().to_string(),
            file: PROBE_FILE.to_string(),
        },
        &format!("namespace {namespace:?}, flow {flow_id:?} and task {task_id:?}"),
    )?;
    Ok(prefix)
}

const PROBE_FILE: &str = "file";

/// `kestra:///{ns}/{flow-slug}/executions/{exec}/inputs/{input}/{file}`
/// (`StorageContext.Input`: the input's name and the file's are NOT slugified).
/// `Err` as [`task_prefix`]'s.
pub fn input_uri(
    namespace: &str,
    flow_id: &str,
    execution: Uuid,
    input: &str,
    file: &str,
) -> Result<String, String> {
    let uri = format!(
        "{}inputs/{input}/{file}",
        execution_base(namespace, flow_id, execution)
    );
    checked(
        &uri,
        execution,
        &Tail::Input {
            input: input.to_string(),
            file: file.to_string(),
        },
        &format!("namespace {namespace:?}, flow {flow_id:?}, input {input:?} and file {file:?}"),
    )?;
    Ok(uri)
}

/// `kestra:///{ns}/{flow-slug}/executions/{exec}/{file}` ([`Tail::Execution`]).
/// `Err` as [`task_prefix`]'s.
pub fn execution_file_uri(
    namespace: &str,
    flow_id: &str,
    execution: Uuid,
    file: &str,
) -> Result<String, String> {
    let uri = format!("{}{file}", execution_base(namespace, flow_id, execution));
    checked(
        &uri,
        execution,
        &Tail::Execution {
            file: file.to_string(),
        },
        &format!("namespace {namespace:?}, flow {flow_id:?} and file {file:?}"),
    )?;
    Ok(uri)
}

/// `{task_prefix}{id_utils_from(abs_path)}-{basename with ' ' -> '+'}` (FilesService.java:94-97).
/// `task_prefix` is one [`task_prefix`] built; `Err` as its, naming the file.
pub fn output_file_uri(task_prefix: &str, abs_path: &str) -> Result<String, String> {
    let basename = abs_path.rsplit('/').next().unwrap_or(abs_path);
    let name = format!("{}-{}", id_utils_from(abs_path), basename.replace(' ', "+"));
    let uri = format!("{task_prefix}{name}");
    let what = format!("output file {abs_path:?}");
    let under = parse(&format!("{task_prefix}{PROBE_FILE}"))
        .map_err(|reason| refusal(&what, task_prefix, &reason))?;
    let Tail::Task {
        task_slug,
        task_run,
        ..
    } = under.tail
    else {
        return Err(refusal(&what, task_prefix, "it is not a task's prefix"));
    };
    checked(
        &uri,
        under.execution,
        &Tail::Task {
            task_slug,
            task_run,
            file: name,
        },
        &what,
    )?;
    Ok(uri)
}

/// `{task_prefix}{rel}`: a file kept under its own relative path -- what
/// Kestra does for `outputDirectory` (`w9a-storage.json`
/// `egret_expected.output_directory.uri_tail`: "the relative path verbatim,
/// NOT the IdUtils hash scheme") and what the plugin bridge's `.egret/out/`
/// does (Task 9). `Err` as [`output_file_uri`]'s, naming the file: a name the
/// parser refuses (`?`, `#`, a backslash, a control character) is the author's.
pub fn verbatim_file_uri(task_prefix: &str, rel: &str) -> Result<String, String> {
    let uri = format!("{task_prefix}{rel}");
    let what = format!("output file {rel:?}");
    let under = parse(&format!("{task_prefix}{PROBE_FILE}"))
        .map_err(|reason| refusal(&what, task_prefix, &reason))?;
    let Tail::Task {
        task_slug,
        task_run,
        ..
    } = under.tail
    else {
        return Err(refusal(&what, task_prefix, "it is not a task's prefix"));
    };
    checked(
        &uri,
        under.execution,
        &Tail::Task {
            task_slug,
            task_run,
            file: rel.to_string(),
        },
        &what,
    )?;
    Ok(uri)
}

/// **Every URI egret builds is read back before it is used**: `parse(built)`
/// must name the same writing execution and the same tail, and print back the
/// same string. A namespace like `a..b` (an empty segment), `a.` (a trailing
/// one) or `x.y.executions.<uuid>` with a flow `tasks` (an earlier
/// `executions/<uuid>` than the writer's) passes publish but would be refused
/// or misread here, and an index writer would then refuse the row inside the
/// commit transaction. Refusing it when the URI is built makes it the
/// author's error, a failed task naming it, instead.
fn checked(built: &str, execution: Uuid, tail: &Tail, what: &str) -> Result<(), String> {
    let parsed = parse(built).map_err(|reason| refusal(what, built, &reason))?;
    if parsed.execution != execution {
        return Err(refusal(
            what,
            built,
            &format!(
                "it reads back as a file of execution {}, not of {execution}",
                parsed.execution
            ),
        ));
    }
    if parsed.tail != *tail || parsed.to_uri() != built {
        return Err(refusal(
            what,
            built,
            "it reads back as a different file of the execution",
        ));
    }
    Ok(())
}

fn refusal(what: &str, built: &str, reason: &str) -> String {
    format!("{what}: the storage URI {built} cannot be read back: {reason}")
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
            let prefix =
                task_prefix(&s("namespace"), &s("flow_id"), exec, &s("task_id"), run).unwrap();
            // Built over egret's UUIDs (the builders read their URI back, and
            // Kestra's base62 ids are not UUIDs), then Kestra's ids put in.
            let kestra_ids = |u: String| {
                u.replace(&exec.to_string(), &s("execution_id"))
                    .replace(&run.to_string(), &s("task_run_id"))
            };
            let got = match s("source").as_str() {
                "outputFiles" => {
                    by_source.0 += 1;
                    kestra_ids(output_file_uri(&prefix, &s("absolute_path")).unwrap())
                }
                "outputDirectory" => {
                    by_source.1 += 1;
                    kestra_ids(format!("{prefix}{}", s("rel_path")))
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

    /// Fix round 1 M3: `\`, `?`, `#` and control characters are refused
    /// anywhere in the path. `%` and `+` are legal: Kestra's measured URIs hold
    /// both, and the scan below is the evidence (a zero beside a count).
    #[test]
    fn parse_refuses_separator_query_fragment_and_control_characters() {
        let id = "0e9a7b4c-3f53-4f0b-9b3e-8d1f2a6c7e10";
        for bad in ["a\\b", "a?b", "a#b", "a\nb", "a\u{7f}b", "a\tb", "a\0b"] {
            let uri = format!("kestra:///ns/f/executions/{id}/inputs/f/{bad}");
            let err = parse(&uri).expect_err(&uri);
            assert!(err.contains("character"), "{uri:?}: {err}");
            let ns = format!("kestra:///n{bad}s/f/executions/{id}/inputs/f/x");
            assert!(parse(&ns).is_err(), "{ns:?}");
        }
        for good in ["a+b.txt", "a%20b.txt", "café.txt", "a b.txt"] {
            let uri = format!("kestra:///ns/f/executions/{id}/inputs/f/{good}");
            assert!(parse(&uri).is_ok(), "{uri}");
        }
        // Every `kestra:///` string in the fixture: how many, and how many hold
        // a refused character.
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/w9a-storage.json"),
        )
        .unwrap();
        let fixture: Value = serde_json::from_str(&text).unwrap();
        let mut uris = std::collections::BTreeSet::new();
        fn walk(v: &Value, out: &mut std::collections::BTreeSet<String>) {
            match v {
                Value::String(s) => {
                    for (i, _) in s.match_indices(SCHEME_PREFIX) {
                        let end = s[i..]
                            .find(|c: char| c.is_whitespace() || "'\")}".contains(c))
                            .map_or(s.len(), |n| i + n);
                        out.insert(s[i..end].to_string());
                    }
                }
                Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                Value::Object(o) => o.values().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        walk(&fixture, &mut uris);
        let refused: Vec<&String> = uris
            .iter()
            .filter(|u| {
                u.chars()
                    .any(|c| matches!(c, '\\' | '?' | '#') || c.is_control())
            })
            .collect();
        assert_eq!((uris.len(), refused.len()), (116, 0), "{refused:?}");
        assert!(uris.iter().any(|u| u.contains('%')) && uris.iter().any(|u| u.contains('+')));
    }

    /// Fix round 1 I1: a namespace that passes publish
    /// (`^[a-z0-9][a-z0-9._-]*`) but whose URI egret's own parser would refuse
    /// or misread is refused when the URI is BUILT, naming the namespace --
    /// never handed to an index writer, where it would be `Error::Internal`.
    #[test]
    fn builders_refuse_a_uri_their_parser_would_refuse_or_misread() {
        let (e, r) = (Uuid::new_v4(), Uuid::new_v4());
        let other = Uuid::new_v4();
        let tricky = format!("x.y.executions.{other}");
        for (ns, flow) in [("a..b", "f"), ("a.", "f"), (tricky.as_str(), "tasks")] {
            let errs = [
                task_prefix(ns, flow, e, "t", r).unwrap_err(),
                input_uri(ns, flow, e, "i", "x.csv").unwrap_err(),
                execution_file_uri(ns, flow, e, "0.ion").unwrap_err(),
            ];
            for err in errs {
                assert!(err.contains(&format!("namespace {ns:?}")), "{err}");
                assert!(err.contains("cannot be read back"), "{err}");
            }
        }
        // The misread is the dangerous one: it parses, as ANOTHER execution.
        let built = format!("kestra:///x/y/executions/{other}/tasks/executions/{e}/inputs/i/x");
        assert_eq!(parse(&built).unwrap().execution, other);
        let err = input_uri(&tricky, "tasks", e, "i", "x").unwrap_err();
        assert!(err.contains(&format!("not of {e}")), "{err}");
        // The controls: the same builders over a plain namespace.
        assert!(task_prefix("a.b", "f", e, "t", r).is_ok());
        assert!(input_uri("a.b", "f", e, "i", "x.csv").is_ok());
        assert!(execution_file_uri("a.b", "f", e, "0.ion").is_ok());
        // An input name or file that would split differently, or a bad character.
        assert!(input_uri("a.b", "f", e, "i/j", "x").is_err());
        assert!(input_uri("a.b", "f", e, "i", "x?y").is_err());
        assert!(execution_file_uri("a.b", "f", e, "d/0.ion").is_err());
        // Output files: the name is built from the pod's path.
        let tp = task_prefix("a.b", "f", e, "t", r).unwrap();
        assert!(output_file_uri(&tp, "/egret/working-dir/ok.txt").is_ok());
        for bad in [
            "/egret/working-dir/a#b",
            "/egret/working-dir/a\nb",
            "/egret/working-dir/a?b",
        ] {
            let err = output_file_uri(&tp, bad).unwrap_err();
            assert!(err.contains("output file"), "{err}");
        }
        assert!(output_file_uri("kestra:///not-a-prefix/", "/x").is_err());
    }

    #[test]
    fn parse_to_uri_round_trips() {
        let (e, r) = (Uuid::new_v4(), Uuid::new_v4());
        let tp = task_prefix("w9a.storage", "Out_Files.Case-", e, "Shell_Task-", r).unwrap();
        let uris = [
            input_uri("a.b.c", "My Flow", e, "file1", "data.csv").unwrap(),
            output_file_uri(&tp, "/egret/working-dir/a b.txt").unwrap(),
            format!("{tp}deep/d.txt"),
            format!("kestra:///w9a/f/executions/{e}/trigger/my-trigger/x.json"),
            execution_file_uri("ns", "f", e, "0.ion").unwrap(),
            // Kestra kept an uploaded `a%20b.txt` verbatim (`w9a-storage.json`
            // `file_input.filenames.percent20`): `%` is legal and not decoded.
            input_uri("ns", "f", e, "f", "a%20b.txt").unwrap(),
            input_uri("ns", "f", e, "f", "%2e%2e").unwrap(),
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
