//! Who a principal set reaches: the audience, with the provenance of every
//! member (governance sub-project 1, Task 3).
//!
//! A `PrincipalSet` names principals the way the store holds them: users,
//! groups, roles and workspaces. An enforcement point resolves users and the
//! `public` group only (D0/D7), so every `WorkspaceMembers`, every `Role` and
//! every non-`public` `Group` is expanded here into the users it stands for,
//! read from a [`Directory`] the caller has already loaded. Every read and
//! every username mint stays with the caller (`egret-perm-compile`'s
//! `members.rs`); this module does no I/O.
//!
//! Each expanded member keeps the source that named it ([`Via`]). A member
//! named twice is kept once, at its first appearance, with the provenance of
//! that first source — the order a list was authored in decides both.

use std::collections::{BTreeMap, BTreeSet};

use egret_perm::{Principal, PrincipalKind, PrincipalSet, PUBLIC_GROUP};
use uuid::Uuid;

/// The member lists an expansion reads, keyed the way each principal kind
/// stores its ref. Every list is the usernames the caller resolved (and
/// minted) for that source, sorted and deduplicated. A referenced source with
/// no entry expands to nobody, exactly like one with an empty entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Directory {
    /// The direct user members of each workspace, by workspace id.
    pub workspaces: BTreeMap<Uuid, Vec<String>>,
    /// The holders of each role, by role name.
    pub roles: BTreeMap<String, Vec<String>>,
    /// The direct members of each group, by group name.
    pub groups: BTreeMap<String, Vec<String>>,
}

/// The name an enforcement point is handed for one member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberName {
    /// A user, by the ref as given (a username; validation is the
    /// compilers' job).
    User(String),
    /// The `public` group: everyone.
    Everyone,
    /// A principal kept as authored because no directory expanded it
    /// ([`audience_from_expanded`]).
    Unexpanded(Principal),
}

/// The source that put a member in the audience.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Via {
    /// Named directly in the set.
    Direct,
    /// A direct user member of this workspace.
    WorkspaceMembers(Uuid),
    /// A holder of this role.
    Role(String),
    /// A direct member of this group.
    Group(String),
}

/// One member of an audience and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: MemberName,
    pub via: Via,
}

/// The expanded form of a `PrincipalSet`: who it includes and who it
/// excepts, each in first-appearance order without duplicates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Audience {
    pub include: Vec<Member>,
    pub except: Vec<Member>,
}

/// The bridge back to a `PrincipalSet` is gone (governance sub-project 1,
/// Task 10): every family reads the audience.
///
/// ```compile_fail
/// let _ = egret_perm_ir::audience::Audience::default().to_principal_set(&Default::default());
/// ```
#[doc(hidden)]
pub struct BridgeRemoved;

/// Expand `set` against `dir`. Each half is expanded on its own.
pub fn audience(set: &PrincipalSet, dir: &Directory) -> Audience {
    Audience {
        include: expand(&set.include, dir),
        except: expand(&set.exclude, dir),
    }
}

/// The audience of a set that is already expanded, with no directory: every
/// principal maps one to one, users to [`MemberName::User`], `public` to
/// [`MemberName::Everyone`], and anything else (a `Role`, `WorkspaceMembers`
/// or other `Group`) is kept as [`MemberName::Unexpanded`]. For callers of
/// `compile_grant` that pass a set the store did not expand.
pub fn audience_from_expanded(set: &PrincipalSet) -> Audience {
    let direct = |list: &[Principal]| -> Vec<Member> {
        list.iter()
            .map(|p| Member {
                name: match p.kind {
                    PrincipalKind::User => MemberName::User(p.r#ref.clone()),
                    PrincipalKind::Group if p.r#ref == PUBLIC_GROUP => MemberName::Everyone,
                    _ => MemberName::Unexpanded(p.clone()),
                },
                via: Via::Direct,
            })
            .collect()
    };
    Audience {
        include: direct(&set.include),
        except: direct(&set.exclude),
    }
}

impl MemberName {
    /// The principal this member is handed to an enforcement point as: a
    /// `User`, the `public` group, or the unexpanded principal as authored.
    /// The families check each member through it, so a member is refused
    /// exactly as the principal it stands for always was.
    pub fn to_principal(&self) -> Principal {
        match self {
            MemberName::User(n) => Principal {
                kind: PrincipalKind::User,
                r#ref: n.clone(),
            },
            MemberName::Everyone => Principal {
                kind: PrincipalKind::Group,
                r#ref: PUBLIC_GROUP.to_string(),
            },
            MemberName::Unexpanded(p) => p.clone(),
        }
    }
}

/// Whether `p` is a group an audience expands: every group except
/// [`PUBLIC_GROUP`], the one name every engine resolves without a group
/// provider (`egret_perm::PUBLIC_GROUP`'s doc) — the compiler's own "applies to
/// everyone" item uses it, and expanding it would turn "everyone" into "every
/// user who has a row naming a group called public". `egret-perm-compile`
/// reads it too, to decide which group names to look up.
pub fn is_expanded_group(p: &Principal) -> bool {
    p.kind == PrincipalKind::Group && p.r#ref != PUBLIC_GROUP
}

/// One half of a set. Explicit principals keep their place; each expansion
/// follows in the directory's order. A name already emitted is not emitted
/// again — a set that names `alice` explicitly AND a workspace, role or group
/// she belongs to must not name her twice.
///
/// A half that names nothing expandable (users and `public` only) maps one to
/// one, duplicates included: the compiler has always handed such a half to
/// the engine as authored.
fn expand(list: &[Principal], dir: &Directory) -> Vec<Member> {
    let touched = list.iter().any(|p| {
        matches!(
            p.kind,
            PrincipalKind::WorkspaceMembers | PrincipalKind::Role
        ) || is_expanded_group(p)
    });
    if !touched {
        return audience_from_expanded(&PrincipalSet {
            include: list.to_vec(),
            exclude: Vec::new(),
        })
        .include;
    }
    let mut out: Vec<Member> = Vec::with_capacity(list.len());
    // Keyed `(kind, ref)` as the principal it lowers to, so `User("public")`
    // and `Everyone` stay distinct.
    let mut seen: BTreeSet<(&'static str, String)> = BTreeSet::new();
    for p in list {
        let expanded: Vec<Member> = match p.kind {
            PrincipalKind::WorkspaceMembers => {
                let id = p.r#ref.parse::<Uuid>().ok();
                let names = id
                    .and_then(|id| dir.workspaces.get(&id))
                    .cloned()
                    .unwrap_or_default();
                if names.is_empty() {
                    tracing::warn!(
                        workspace_ref = %p.r#ref,
                        "a workspace_members principal expanded to no engine principal — it \
                         names no workspace, or the workspace has no direct user members with \
                         a live allow grant"
                    );
                }
                // `names` is empty whenever `id` is `None`.
                users(names, Via::WorkspaceMembers(id.unwrap_or_default()))
            }
            PrincipalKind::Role => {
                let names = dir.roles.get(&p.r#ref).cloned().unwrap_or_default();
                if names.is_empty() {
                    tracing::warn!(
                        role = %p.r#ref,
                        "a role principal expanded to no engine principal — no such role, or \
                         nobody holds it with a live allow grant. The \
                         Ranger `roles` bucket is NOT used as a fallback: D0 measured that it \
                         matches nobody, so naming the role there would be a control that does \
                         nothing (in an include) or one that denies everybody (in a \
                         denyException)"
                    );
                }
                users(names, Via::Role(p.r#ref.clone()))
            }
            PrincipalKind::Group if is_expanded_group(p) => {
                let names = dir.groups.get(&p.r#ref).cloned().unwrap_or_default();
                if names.is_empty() {
                    tracing::warn!(
                        group = %p.r#ref,
                        "a group principal expanded to no engine principal — no such group, or \
                         it has no members. The Ranger `groups` bucket is NOT used as a \
                         fallback: D7 measured that no engine egret provisions resolves a \
                         group, so naming it there would be a control that does nothing"
                    );
                }
                users(names, Via::Group(p.r#ref.clone()))
            }
            PrincipalKind::Group => vec![Member {
                name: MemberName::Everyone,
                via: Via::Direct,
            }],
            PrincipalKind::User => vec![Member {
                name: MemberName::User(p.r#ref.clone()),
                via: Via::Direct,
            }],
        };
        for m in expanded {
            let key = m.name.to_principal();
            if seen.insert((key.kind.as_str(), key.r#ref)) {
                out.push(m);
            }
        }
    }
    out
}

fn users(names: Vec<String>, via: Via) -> Vec<Member> {
    names
        .into_iter()
        .map(|n| Member {
            name: MemberName::User(n),
            via: via.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::{Principal, PrincipalKind, PrincipalSet};

    fn p(kind: PrincipalKind, r: &str) -> Principal {
        Principal {
            kind,
            r#ref: r.into(),
        }
    }

    #[test]
    fn a_plain_user_list_is_left_alone_duplicates_included() {
        let set = PrincipalSet {
            include: vec![p(PrincipalKind::User, "a"), p(PrincipalKind::User, "a")],
            exclude: vec![],
        };
        let out = expanded(&set, &Directory::default());
        assert_eq!(
            out, set,
            "a half naming nothing expandable is handed on as authored, duplicates included"
        );
    }

    #[test]
    fn expansion_is_first_appearance_ordered_and_deduped() {
        let ws = uuid::Uuid::from_u128(7);
        let mut dir = Directory::default();
        dir.workspaces.insert(ws, vec!["b".into(), "c".into()]);
        dir.roles
            .insert("analyst".into(), vec!["c".into(), "d".into()]);
        dir.groups.insert("eng".into(), vec!["a".into()]);
        let set = PrincipalSet {
            include: vec![
                p(PrincipalKind::User, "a"),
                p(PrincipalKind::WorkspaceMembers, &ws.to_string()),
                p(PrincipalKind::Role, "analyst"),
                p(PrincipalKind::Group, "eng"),
                p(PrincipalKind::Group, "public"),
            ],
            exclude: vec![],
        };
        let a = audience(&set, &dir);
        let names: Vec<_> = a.include.iter().map(|m| m.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                MemberName::User("a".into()),
                MemberName::User("b".into()),
                MemberName::User("c".into()),
                MemberName::User("d".into()),
                MemberName::Everyone,
            ]
        );
        assert_eq!(a.include[1].via, Via::WorkspaceMembers(ws));
        assert_eq!(a.include[3].via, Via::Role("analyst".into()));
    }

    #[test]
    fn an_unparseable_workspace_ref_expands_to_nobody() {
        let set = PrincipalSet {
            include: vec![p(PrincipalKind::WorkspaceMembers, "not-a-uuid")],
            exclude: vec![],
        };
        assert!(audience(&set, &Directory::default()).include.is_empty());
    }

    // Moved from `egret-perm-compile`'s `members.rs` tests (Task 3), where
    // they exercised `substitute`.

    fn user(name: &str) -> Principal {
        p(PrincipalKind::User, name)
    }

    fn group(name: &str) -> Principal {
        p(PrincipalKind::Group, name)
    }

    fn ws_principal(id: uuid::Uuid) -> Principal {
        p(PrincipalKind::WorkspaceMembers, &id.to_string())
    }

    fn principals(a: &Audience) -> PrincipalSet {
        let half = |m: &[Member]| m.iter().map(|m| m.name.to_principal()).collect();
        PrincipalSet {
            include: half(&a.include),
            exclude: half(&a.except),
        }
    }

    fn expanded(set: &PrincipalSet, dir: &Directory) -> PrincipalSet {
        principals(&audience(set, dir))
    }

    /// J-F3 Task 1b: a group expands to its members on BOTH sides, in place,
    /// deduplicated against an explicitly named member — and an empty group
    /// leaves nothing behind, never an entry naming the group.
    #[test]
    fn a_group_expands_to_its_members_and_an_empty_group_to_nothing() {
        let mut dir = Directory::default();
        dir.groups
            .insert("analysts".into(), vec!["alice".into(), "bob".into()]);
        dir.groups.insert("nobody".into(), Vec::new());
        let set = PrincipalSet {
            include: vec![user("alice"), group("analysts"), group("nobody")],
            exclude: vec![group("analysts")],
        };
        let out = expanded(&set, &dir);
        assert_eq!(out.include, vec![user("alice"), user("bob")]);
        assert_eq!(out.exclude, vec![user("alice"), user("bob")]);
    }

    /// `public` is the one group every engine resolves; it is never expanded,
    /// even if a `groups` row carries that name and has members.
    #[test]
    fn the_public_group_is_never_expanded() {
        let mut dir = Directory::default();
        dir.groups
            .insert(PUBLIC_GROUP.to_string(), vec!["mallory".into()]);
        let set = PrincipalSet {
            include: vec![group(PUBLIC_GROUP)],
            exclude: vec![],
        };
        assert_eq!(expanded(&set, &dir).include, vec![group(PUBLIC_GROUP)]);
        assert_eq!(
            audience(&set, &dir).include,
            vec![Member {
                name: MemberName::Everyone,
                via: Via::Direct
            }]
        );
    }

    #[test]
    fn a_workspace_expands_to_its_members_in_place() {
        let ws = uuid::Uuid::new_v4();
        let mut dir = Directory::default();
        dir.workspaces
            .insert(ws, vec!["alice".into(), "bob-2".into()]);
        let set = PrincipalSet {
            include: vec![user("carol"), ws_principal(ws)],
            exclude: vec![],
        };
        assert_eq!(
            expanded(&set, &dir).include,
            vec![user("carol"), user("alice"), user("bob-2")],
            "explicit principals keep their place; the expansion follows, sorted"
        );
    }

    #[test]
    fn an_already_named_member_is_not_named_twice() {
        let ws = uuid::Uuid::new_v4();
        let mut dir = Directory::default();
        dir.workspaces.insert(ws, vec!["alice".into()]);
        let set = PrincipalSet {
            include: vec![user("alice"), ws_principal(ws)],
            exclude: vec![],
        };
        let a = audience(&set, &dir);
        assert_eq!(principals(&a).include, vec![user("alice")]);
        assert_eq!(a.include[0].via, Via::Direct, "the first source wins");
    }

    /// The `exclude` half is the one that fails OPEN when it is got wrong: a
    /// subtract that evaporates grants MORE than the model states. Expanding
    /// it to zero principals must leave the list EMPTY (so `compile_grant`
    /// emits no exception block at all), never an entry naming nobody.
    #[test]
    fn an_empty_workspace_expands_to_nothing_rather_than_to_an_empty_item() {
        let ws = uuid::Uuid::new_v4();
        let mut dir = Directory::default();
        dir.workspaces.insert(ws, Vec::new());
        let set = PrincipalSet {
            include: vec![ws_principal(ws)],
            exclude: vec![ws_principal(ws)],
        };
        let out = expanded(&set, &dir);
        assert!(out.include.is_empty());
        assert!(out.exclude.is_empty());
    }

    /// Each half is judged on its own: an expandable include does not make a
    /// plain exclude lose its duplicates, as `substitute_one` never did.
    #[test]
    fn each_half_keeps_its_own_early_return() {
        let mut dir = Directory::default();
        dir.roles.insert("r".into(), vec!["x".into()]);
        let set = PrincipalSet {
            include: vec![p(PrincipalKind::Role, "r")],
            exclude: vec![user("y"), user("y")],
        };
        let out = expanded(&set, &dir);
        assert_eq!(out.include, vec![user("x")]);
        assert_eq!(out.exclude, vec![user("y"), user("y")]);
    }

    /// Without a directory nothing expands and every principal maps one to
    /// one, so the members name the set unchanged.
    #[test]
    fn audience_from_expanded_keeps_unexpanded_members() {
        let set = PrincipalSet {
            include: vec![user("a"), p(PrincipalKind::Role, "r"), group(PUBLIC_GROUP)],
            exclude: vec![p(PrincipalKind::WorkspaceMembers, "w")],
        };
        let a = audience_from_expanded(&set);
        let names: Vec<_> = a.include.iter().map(|m| m.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                MemberName::User("a".into()),
                MemberName::Unexpanded(p(PrincipalKind::Role, "r")),
                MemberName::Everyone,
            ]
        );
        assert_eq!(
            a.except[0].name,
            MemberName::Unexpanded(p(PrincipalKind::WorkspaceMembers, "w"))
        );
        assert_eq!(principals(&a), set);
    }
}
