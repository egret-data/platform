# Monitor sees every user's queries on a Ranger engine

## Problem

The native monitor lists queries through Trino's `/v1/query`, which Trino
filters by access control. On every coordinator that is not JWT-only it signs
in as `egret-monitor`, which holds no grant on other users, so the Ranger plugin
hides their queries and the Monitor tab shows none.

## Mechanism (confirmed in the plugin source)

`RangerSystemAccessControl.checkCanViewQueryOwnedBy` and
`checkCanKillQueryOwnedBy` both call `hasPermission(createUserResource(owner),
context, IMPERSONATE)`: the `impersonate` access on the `trinouser` resource.
`filterViewQueryOwnedBy` is a no-op.

## Fix

A compiled infrastructure policy `egret-monitor-view-queries`: `egret-monitor`
may `impersonate` `trinouser: *`. Emitted by `lower_infra` as
`InfraKind::MonitorViewQueries` when the new `InfraInputs::native_monitor` is
set; `native_monitor` is "some bound, egret-provisioned Trino engine is not
JWT-only", computed by the shared pure predicate
`egret_core::engine_delegation::monitor_signs_in_as_service_user`, built on the
new `is_jwt_only_coordinator` that `Engine::is_jwt_only_coordinator` now also
delegates to. `monitor.rs` sign-in is untouched.

Golden fixtures of open/password/impersonation Trino profiles gain the policy;
JWT-only and StarRocks fixtures do not change.
