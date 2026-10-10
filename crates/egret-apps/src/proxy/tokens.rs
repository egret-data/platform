//! One query token per grant, minted by the proxy, and the count of WebSockets that keep it
//! alive (D19).
//!
//! The token is the plaintext the pod sees in `X-Egret-App-Token`; only its hash is stored, so
//! the proxy keeps the plaintext in memory. A miss — a restart, another replica, a token that
//! was revoked — simply mints a fresh one, which is what makes an iframe reload work: the old
//! page's WebSocket closing no longer strands the new page.
//!
//! The WebSocket count is per replica. With several replicas the last close on THIS replica
//! revokes the grant's tokens even if another replica holds a socket for the same grant, and
//! that socket's next request re-mints. The backstop for a socket that never closes is the
//! token's idle expiry (30 min).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use egret_core::{Result, UserId};
use uuid::Uuid;

use crate::grant::Grant;
use crate::session::{AppSessionRepo, NewAppSession};

/// How long a cached token is trusted without asking the database whether it is still live.
/// Asking also slides its idle expiry, so a person editing for an hour without running a
/// query does not come back to "session expired".
const RECHECK: Duration = Duration::from_secs(60);

struct Entry {
    token: String,
    checked: Instant,
    open_ws: usize,
}

#[derive(Default)]
pub(crate) struct TokenBook {
    map: Mutex<HashMap<Uuid, Entry>>,
    /// D24/`apps.pod.run_session_cap`: open WebSockets of ONE viewer's run pod, keyed by
    /// `(viewer, version)` rather than grant id — a page reload can mint a fresh grant, but it is
    /// still the same pod and the same cap. Not a multi-tenant limit: D24 makes one pod one person.
    run_sessions: Mutex<HashMap<(UserId, Uuid), u32>>,
}

impl TokenBook {
    /// The live token of `grant`, minting one if there is none.
    pub(crate) async fn token_for(
        &self,
        grant: &Grant,
        sessions: &AppSessionRepo,
        idle: Duration,
        absolute: Duration,
    ) -> Result<String> {
        let cached = {
            let map = self.map.lock().unwrap();
            map.get(&grant.id).map(|e| (e.token.clone(), e.checked))
        };
        if let Some((token, checked)) = cached.filter(|(t, _)| !t.is_empty()) {
            if checked.elapsed() < RECHECK {
                return Ok(token);
            }
            if sessions.resolve(&token, idle).await?.is_some() {
                if let Some(e) = self.map.lock().unwrap().get_mut(&grant.id) {
                    e.checked = Instant::now();
                }
                return Ok(token);
            }
        }
        let token = sessions
            .mint(
                &NewAppSession {
                    user_id: grant.user_id,
                    workspace_id: grant.workspace_id,
                    app_id: grant.app_id,
                    version_id: grant.version_id,
                    mode: grant.mode,
                    login_sid: grant.login_sid,
                    grant_id: Some(grant.id),
                    impersonator: grant.impersonator,
                },
                idle,
                absolute,
            )
            .await?;
        let mut map = self.map.lock().unwrap();
        let open_ws = map.get(&grant.id).map_or(0, |e| e.open_ws);
        map.insert(
            grant.id,
            Entry {
                token: token.clone(),
                checked: Instant::now(),
                open_ws,
            },
        );
        Ok(token)
    }

    pub(crate) fn ws_opened(&self, grant: Uuid) {
        let mut map = self.map.lock().unwrap();
        match map.get_mut(&grant) {
            Some(e) => e.open_ws += 1,
            None => {
                // A WebSocket always mints first, so this is only reachable if the entry was
                // dropped between the two; count it anyway.
                map.insert(
                    grant,
                    Entry {
                        token: String::new(),
                        checked: Instant::now()
                            .checked_sub(RECHECK * 2)
                            .unwrap_or_else(Instant::now),
                        open_ws: 1,
                    },
                );
            }
        }
    }

    /// True when that was the grant's last open WebSocket on this replica: the caller revokes
    /// the grant's tokens. The cache entry goes with it.
    pub(crate) fn ws_closed(&self, grant: Uuid) -> bool {
        let mut map = self.map.lock().unwrap();
        let Some(e) = map.get_mut(&grant) else {
            return true;
        };
        e.open_ws = e.open_ws.saturating_sub(1);
        if e.open_ws == 0 {
            map.remove(&grant);
            true
        } else {
            false
        }
    }

    /// Claims one of that `(viewer, version)` pod's `cap` session slots. `false` means the pod is
    /// already at capacity and the caller must refuse the upgrade before it ever dials upstream.
    pub(crate) fn run_ws_try_open(&self, user: UserId, version: Uuid, cap: u32) -> bool {
        let mut m = self.run_sessions.lock().unwrap();
        let n = m.entry((user, version)).or_insert(0);
        if *n >= cap {
            return false;
        }
        *n += 1;
        true
    }

    /// Releases a slot claimed by `run_ws_try_open`.
    pub(crate) fn run_ws_closed(&self, user: UserId, version: Uuid) {
        let mut m = self.run_sessions.lock().unwrap();
        if let Some(n) = m.get_mut(&(user, version)) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(&(user, version));
            }
        }
    }
}
