# StarRocks engine posture: unbound engines and legacy engines (design)

**Status:** approved by the user on 2026-11-09, in two sections (KI-336, then legacy engines).

**Package:** B of the StarRocks follow-ups, done after governance SP2. The order is B, then A (the
audit platform side and the Query log tab), then C (a probe of the paths no one has measured yet).
A and C get their own specs.

## Problem

Two kinds of StarRocks engines escape what egret enforces today.

1. **Unbound engines (KI-336).** A StarRocks engine provisioned without a policy profile stays
   without one, even when Ranger is on:
   - egret creates a profile automatically only for a Trino engine;
   - **Add engine** has no profile picker for StarRocks;
   - such an engine enforces none of egret's grants, masks or row filters, and nothing says so.
2. **Legacy engines.** An engine created before platform #826 and #832 may still run:
   - **an official StarRocks FE image, or an egret image before `4.1.7-egret.4`.** Either one
     still has the holes egret's fork closes:
     - KI-340, Iceberg metadata tables served without Ranger;
     - KI-345, no mask or row filter on prepared statements, which affects Data API endpoints
       with parameters;
     - the HTTP SQL API, Arrow Flight SQL, FILES(), materialized views, EXPLAIN COSTS and
       BROKER LOAD/EXPORT/OUTFILE holes;
   - **a passwordless FE `root`.** Every engine created since the root-password task gets a
     strong one (`ensure_root_password_secret`). An older engine has no `root_secret_id`, so
     anyone who can reach FE port 9030 can log in as `root`.

   Today the image holes show up only as a warning on the engine's own page
   (`StarRocksMetadataTablesWarning`). There is no list of affected engines, and no way to fix
   one short of hand-editing `feImage` in Settings. The missing password is not shown anywhere.

## Part 1: KI-336, StarRocks gets a profile like Trino

### Create

`create_starrocks_engine` decides its profile the way a Trino create does. `plan_trino_profile`
becomes `plan_profile(st, name, engine_type, requested)`, and both create paths call it. It
returns one of three plans:
- `Existing(id)` when the request names a profile that exists;
- `Unbound` when Ranger is off and no profile was named. This is today's behaviour;
- `Create` when Ranger is on and no profile named `<name>` exists. `bind_trino_profile` becomes
  `bind_profile`.
  - For StarRocks it creates the profile `<name>` with type `starrocks`, and **no** tag service.
    Migration 0176 forbids `tagService` on a non-Trino profile.
  - The bind goes through `bind_profile_guarded`, as for Trino.

There are two refusals:
- a same-name profile of another type gets 409;
- a same-name profile that another engine already binds gets 409.

`profile_type_conflict` names the expected type in its message ("not starrocks"), not a fixed
"not trino". A failed create is undone through the existing `CreateUndo`, which deletes the
profile only if this create made it.

### Add engine form

**Add engine** has no profile picker today: a Trino create makes its profile automatically
(`frontend/src/routes/control-plane/EngineCreate.tsx` has none). The StarRocks **Policy profile**
select is therefore a new control, and Trino's form is unchanged. It lists only profiles of type
`starrocks` that no engine binds, and offers "Create a profile named after the engine" as its
default.

### Unbound engines

When `ranger.enabled` is on, a StarRocks engine with no `authz_service_id` shows a warning, both
on its page and as a badge in the engine list:

> Not governed: this engine is bound to no policy profile, so egret's grants, masks and row
> filters do not reach it. Bind it to a StarRocks profile in its settings.

The frontend cannot tell whether Ranger is on, so the backend decides: the engine DTO carries
`notGoverned: bool`, true only when Ranger is on, the engine is StarRocks and it has no profile.
The page and the badge read that field.

The fix is the existing route, `PUT /api/control-plane/engines/{id}` with `authzServiceId`. When
Ranger is off, no warning is shown, because no engine is governed then.

## Part 2: legacy engines, a badge and an upgrade action

### When an engine needs an upgrade

This applies only to a **provisioned** StarRocks engine. An external engine's image is not
egret's to change. The engine DTO gains these fields:
- `securityUpgradeNeeded: bool`;
- `securityUpgradeReasons: ("unpatchedImage" | "noRootPassword")[]`, in that order;
- `securityUpgradeFeImage`: the FE image the upgrade would set, absent when it would set none;
- `securityUpgradeBlockedRelease`: the newer release that keeps the current image (the downgrade
  block), absent otherwise.

The last two let the confirmation dialog show the target before the call.

The reasons:
- **`unpatchedImage`**: the stored FE image does not close every hole. The rule moves from the
  frontend (`isPatchedStarRocksImage`) into `egret-perm::engine_images`, so there is one copy of
  it, and the frontend reads the DTO. An image is patched when:
  - its digest equals the digest of a `KNOWN_ENGINE_IMAGES` egret FE row at or after the first
    complete level. This counts a mirrored image in a private registry, as an air-gapped install
    would have;
  - or it is `ghcr.io/egret-data/egret-starrocks-fe:<release>-egret.<N>`, with release 4.1.7 and
    N of 4 or more, or any later release.

  Anything else is unpatched, including an image egret does not know.
- **`noRootPassword`**: the engine has no `root_secret_id`.

### What the UI shows

- **The engine list** shows a "Security upgrade needed" badge, styled like Trino's "Re-apply
  needed".
- **The engine page** shows a warning that names each reason and has the **Upgrade security**
  button (`engine:update`).
  - While this warning shows, the older `ProvisionedStarRocksMetadataTablesWarning` does not, so
    the page carries one message, not two.
  - For a bound engine that needs no upgrade, nothing changes: that warning is already hidden on
    a patched image.

### The upgrade action

The route is `POST /api/control-plane/engines/{id}/security-upgrade`, guarded by `engine:update`.
It returns 409 for an external engine or a non-StarRocks engine.

**Confirmation dialog.** Before anything changes, the dialog lists what will change:
- `FE image: <current> → <default>` and `CN image: <current> → <default>`, from
  `DEFAULT_ENGINE_IMAGES`;
- "root password will be set".

It warns that:
- the FE and CN pods restart;
- a client outside egret that logs in as `root` with no password is refused afterwards.

**Steps**, each one idempotent, so a repeat finishes a partial run:
1. **No root password:** `ensure_root_password_secret`, which stores `root_secret_id`.
2. **Unpatched image, and no downgrade:** set `starrocksConfig.feImage` and `cnImage` to the
   defaults.
3. **Apply the engine.** This is the same apply a settings change runs. The provisioner renders
   the `{name}-initpwd` Job, whose script sets the password on the running FE through a
   passwordless `root` login; it is a no-op once the password is set.

**Downgrade guard.** If the FE image's release is newer than the default fork image's release
(for example an official 4.1.8), the image is not changed, because StarRocks FE metadata cannot
be downgraded. The root password is still set. The answer and the dialog say:

> No patched image for release `<x>` yet: the image was not changed.

`unpatchedImage` then stays.

**Answer:** the engine, with recomputed `securityUpgradeNeeded` and `securityUpgradeReasons`, plus
`imageChanged: bool` and `rootPasswordSet: bool`.

### Rollout

Nothing changes on its own at upgrade time: no migration rewrites images or mints passwords. An
admin presses the button, because the FE restarts.

## Testing

Every behaviour change gets a test that fails without it, and a mutant of the change that the
test kills, recorded in the task report.

**Unit:**
- the patched-image rule:
  - by tag: 4.1.7-egret.3 is unpatched, 4.1.7-egret.4 is patched, 4.1.8-egret.1 is patched;
  - by digest, for a mirrored repository;
  - an unknown image is unpatched;
- the downgrade guard;
- the reason list and its order;
- `plan_profile` for StarRocks.

**DB tests:**
- **StarRocks create:**
  - with Ranger on, the profile is created and bound;
  - with Ranger off, the engine is unbound;
  - a named profile is bound;
  - a same-name profile of another type gets 409;
  - a failed create removes only the profile it made.
- **Security upgrade:**
  - a legacy engine gets the images and `root_secret_id`, and a second call changes nothing;
  - the downgrade guard sets the password and keeps the image;
  - an external engine gets 409;
  - a principal without `engine:update` gets 403.

**Render test:** after the upgrade, the provision request carries the root password, and the
`{name}-initpwd` Job and Secret render.

**Frontend (vitest):**
- the profile picker on the StarRocks branch;
- the "Not governed" warning, shown only with Ranger on;
- the badge;
- the upgrade warning and dialog, including the downgrade text;
- the old warning hidden while the upgrade warning shows.

**Live check on kind:**
1. Create a StarRocks engine as a legacy engine would be: official 4.1.6 images and no root
   secret.
2. Press **Upgrade security**.
3. Check that:
   - `mysql -h <fe> -P 9030 -u root` with no password is refused;
   - an egret query on the engine still runs;
   - the badge and the warning are gone;
   - the FE image is the default.

## Documentation (egret-data/docs)

- **Engine detail:** the badge, the warning, the **Upgrade security** action and what it
  restarts; the "Not governed" warning.
- **Add engine:** the StarRocks profile picker.
- **Upgrade notes:** check the engine list for "Security upgrade needed" after upgrading egret.
- **Known issues:** KI-336 fixed. KI-340 and KI-345 point to the upgrade action for engines on
  older images.
- **REST API:** `POST /engines/{id}/security-upgrade`, and the new DTO fields.
- **Screenshots:** the badge, the warning, the dialog and the picker.

## Out of scope

- External StarRocks engines: their image and root password are their operator's.
- The audit platform side (package A) and the probe (package C).
- Native tag policies for StarRocks.
