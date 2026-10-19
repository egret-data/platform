//! One judging function for the Ranger plugin download ledger
//! (`policy.ranger_plugin_downloads`), shared by the Governance Overview's
//! drift findings and the engine page's Plugin tab (DG #4a, spec
//! `docs/en/specs/2026-10-19-dg-engine-policy-design.md` §4.1, EP16), so the
//! two never disagree.
//!
//! The rules are the Overview's, moved (`docs/en/plans/2026-10-18-dg-overview-live.md`
//! §1.1):
//! - liveness reads the NEWEST `policies` poll per profile, so the rows a
//!   rescheduled pod leaves behind never count;
//! - a report of `lastKnownVersion = -1` is "not yet reported": it neither
//!   confirms nor counts as behind;
//! - a kind with no download within 24 h is not judged (StarRocks never
//!   downloads tags); a kind polled 5 min–24 h ago while policies are live is
//!   quiet (review M6).
use chrono::{DateTime, Utc};
use sqlx::PgConnection;

/// A profile no plugin has polled for this long is quiet.
pub(crate) const QUIET_SECS: i64 = 5 * 60;
/// … and for this long, silent.
pub(crate) const SILENT_SECS: i64 = 24 * 3600;
/// A version not confirmed this long after it was set is behind. Measured
/// (Overview live doc §1.1): confirmation takes two polls, about 60 s at the
/// rendered 30 s poll.
pub(crate) const BEHIND_SECS: i64 = 90;

/// A profile's counters and when they last moved (`updated_at`, U).
#[derive(Debug, Clone)]
pub(crate) struct ProfileVersions {
    pub policy_version: i64,
    pub tag_version: i64,
    pub updated_at: DateTime<Utc>,
}

/// One row of `policy.ranger_plugin_downloads`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct LedgerRow {
    pub service_id: i64,
    pub plugin_id: String,
    pub kind: String,
    pub last_known_version: Option<i64>,
    pub served_version: Option<i64>,
    pub at: DateTime<Utc>,
}

/// Whether any plugin polls the profile's policies, by the newest poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Liveness {
    Live,
    /// The newest `policies` poll is 5 min to 24 h old.
    Quiet {
        age_secs: i64,
    },
    /// No `policies` poll within 24 h.
    Silent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotReported {
    /// No row of this kind within 24 h.
    NoDownload,
    /// Polled since the version was set, but every report is `-1`.
    VersionUnreported,
}

/// What one kind (`policies` or `tags`) of a profile is judged to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KindState {
    /// A plugin polled since U reports holding the current version, and none
    /// reports less.
    UpToDate,
    /// The version was set under [`BEHIND_SECS`] ago and is not yet confirmed.
    Pending,
    /// The current `version` is not reported held [`BEHIND_SECS`] after U.
    Behind {
        version: i64,
    },
    Quiet {
        age_secs: i64,
    },
    Silent,
    NotReported(NotReported),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Judgement {
    pub liveness: Liveness,
    pub policies: KindState,
    pub tags: KindState,
}

/// Judge one profile's ledger rows at `now`.
pub(crate) fn judge_plugins(
    p: &ProfileVersions,
    rows: &[&LedgerRow],
    now: DateTime<Utc>,
) -> Judgement {
    let age = |at: DateTime<Utc>| (now - at).num_seconds();
    let newest_age = |kind: &str| {
        rows.iter()
            .filter(|r| r.kind == kind)
            .map(|r| age(r.at))
            .min()
    };
    // Liveness: the NEWEST `policies` poll. Every plugin polls policies.
    let liveness = match newest_age("policies") {
        None => Liveness::Silent,
        Some(a) if a >= SILENT_SECS => Liveness::Silent,
        Some(a) if a >= QUIET_SECS => Liveness::Quiet { age_secs: a },
        Some(_) => Liveness::Live,
    };
    let carried = match liveness {
        Liveness::Silent => Some(KindState::Silent),
        Liveness::Quiet { age_secs } => Some(KindState::Quiet { age_secs }),
        Liveness::Live => None,
    };
    let kind_state = |kind: &str, v: i64| -> KindState {
        if let Some(s) = carried {
            return s;
        }
        match newest_age(kind) {
            None => return KindState::NotReported(NotReported::NoDownload),
            Some(a) if a >= SILENT_SECS => return KindState::NotReported(NotReported::NoDownload),
            // Polled within 24 h but not within 5 min, while policies are
            // live: quiet, and its version is not judged (review M6).
            Some(a) if a >= QUIET_SECS => return KindState::Quiet { age_secs: a },
            Some(_) => {}
        }
        // Rows polled at or after U, within 5 min (a dead pod's id is not
        // counted); reports of `-1` neither confirm nor refute.
        let since: Vec<&&LedgerRow> = rows
            .iter()
            .filter(|r| r.kind == kind && r.at >= p.updated_at && age(r.at) < QUIET_SECS)
            .collect();
        let reported: Vec<i64> = since
            .iter()
            .filter_map(|r| r.last_known_version.filter(|v| *v >= 0))
            .collect();
        let lagging = reported.iter().any(|held| *held < v);
        if !reported.is_empty() && !lagging {
            KindState::UpToDate
        } else if age(p.updated_at) < BEHIND_SECS {
            KindState::Pending
        } else if lagging || since.is_empty() {
            KindState::Behind { version: v }
        } else {
            KindState::NotReported(NotReported::VersionUnreported)
        }
    };
    Judgement {
        liveness,
        policies: kind_state("policies", p.policy_version),
        tags: kind_state("tags", p.tag_version),
    }
}

/// The ledger rows of `service_ids`, newest first.
pub(crate) async fn ledger_rows_via(
    c: &mut PgConnection,
    service_ids: &[i64],
) -> Result<Vec<LedgerRow>, sqlx::Error> {
    sqlx::query_as(
        "SELECT service_id, plugin_id, kind, last_known_version, served_version, at \
           FROM policy.ranger_plugin_downloads \
          WHERE service_id = ANY($1) ORDER BY at DESC, service_id, kind, plugin_id",
    )
    .bind(service_ids)
    .fetch_all(&mut *c)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 19, 12, 0, 0).unwrap()
    }
    fn ago(s: i64) -> DateTime<Utc> {
        now() - Duration::seconds(s)
    }
    fn profile(v: i64, tv: i64, set_secs_ago: i64) -> ProfileVersions {
        ProfileVersions {
            policy_version: v,
            tag_version: tv,
            updated_at: ago(set_secs_ago),
        }
    }
    fn row(plugin: &str, kind: &str, known: i64, secs_ago: i64) -> LedgerRow {
        LedgerRow {
            service_id: 1,
            plugin_id: plugin.into(),
            kind: kind.into(),
            last_known_version: Some(known),
            served_version: Some(known),
            at: ago(secs_ago),
        }
    }
    fn judge(p: &ProfileVersions, rows: &[LedgerRow]) -> Judgement {
        let refs: Vec<&LedgerRow> = rows.iter().collect();
        judge_plugins(p, &refs, now())
    }

    #[test]
    fn live_and_confirmed_is_up_to_date() {
        let p = profile(7, 3, 600);
        let j = judge(&p, &[row("a", "policies", 7, 10), row("a", "tags", 3, 20)]);
        assert_eq!(j.liveness, Liveness::Live);
        assert_eq!(j.policies, KindState::UpToDate);
        assert_eq!(j.tags, KindState::UpToDate);
    }

    #[test]
    fn only_minus_one_since_the_bump_is_version_unreported() {
        let p = profile(7, 3, 600);
        let j = judge(
            &p,
            &[row("a", "policies", -1, 10), row("a", "tags", -1, 10)],
        );
        assert_eq!(
            j.policies,
            KindState::NotReported(NotReported::VersionUnreported)
        );
        assert_eq!(
            j.tags,
            KindState::NotReported(NotReported::VersionUnreported)
        );
    }

    #[test]
    fn a_bump_30_s_ago_is_pending() {
        let p = profile(8, 3, 30);
        let j = judge(&p, &[row("a", "policies", 7, 5), row("a", "tags", 3, 5)]);
        assert_eq!(j.policies, KindState::Pending);
    }

    #[test]
    fn an_old_version_reported_since_the_bump_is_behind() {
        let p = profile(8, 3, 120);
        let j = judge(&p, &[row("a", "policies", 7, 5), row("a", "tags", 3, 5)]);
        assert_eq!(j.policies, KindState::Behind { version: 8 });
        assert_eq!(j.tags, KindState::UpToDate);
    }

    #[test]
    fn no_poll_since_the_bump_is_behind() {
        // The newest poll (200 s ago) predates the bump (120 s ago) but is live.
        let p = profile(8, 3, 120);
        let j = judge(&p, &[row("a", "policies", 7, 200)]);
        assert_eq!(j.liveness, Liveness::Live);
        assert_eq!(j.policies, KindState::Behind { version: 8 });
    }

    #[test]
    fn an_old_plugin_ids_row_before_the_bump_is_ignored() {
        let p = profile(8, 3, 120);
        let j = judge(
            &p,
            &[
                row("old", "policies", 2, 4000),
                row("new", "policies", 8, 5),
            ],
        );
        assert_eq!(j.liveness, Liveness::Live);
        assert_eq!(j.policies, KindState::UpToDate);
    }

    #[test]
    fn newest_poll_10_min_ago_is_quiet_for_both_kinds() {
        let p = profile(8, 3, 3600);
        let j = judge(
            &p,
            &[row("a", "policies", 8, 600), row("a", "tags", 3, 600)],
        );
        assert_eq!(j.liveness, Liveness::Quiet { age_secs: 600 });
        assert_eq!(j.policies, KindState::Quiet { age_secs: 600 });
        assert_eq!(j.tags, KindState::Quiet { age_secs: 600 });
    }

    #[test]
    fn newest_poll_25_h_ago_is_silent() {
        let p = profile(8, 3, 3600 * 30);
        let j = judge(&p, &[row("a", "policies", 8, 25 * 3600)]);
        assert_eq!(j.liveness, Liveness::Silent);
        assert_eq!(j.policies, KindState::Silent);
        assert_eq!(j.tags, KindState::Silent);
        // No row at all is silent too.
        assert_eq!(judge(&p, &[]).liveness, Liveness::Silent);
    }

    #[test]
    fn tags_quiet_while_policies_are_live() {
        let p = profile(8, 3, 3600);
        let j = judge(&p, &[row("a", "policies", 8, 5), row("a", "tags", 3, 1200)]);
        assert_eq!(j.liveness, Liveness::Live);
        assert_eq!(j.policies, KindState::UpToDate);
        assert_eq!(j.tags, KindState::Quiet { age_secs: 1200 });
    }

    #[test]
    fn a_profile_with_no_tags_rows_reports_no_download() {
        let p = profile(8, 1, 3600);
        let j = judge(&p, &[row("sr", "policies", 8, 5)]);
        assert_eq!(j.policies, KindState::UpToDate);
        assert_eq!(j.tags, KindState::NotReported(NotReported::NoDownload));
    }
}
