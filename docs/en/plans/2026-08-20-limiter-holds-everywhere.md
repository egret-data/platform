# Make the Rate Limiter Hold On Every Path

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** The rate limiter cannot be reset by a caller, and its memory cannot be grown without bound by one.

**Architecture:** Stop trusting a caller-supplied header at the SPA-fronted edge, and let the limiter forget callers it is no longer tracking.

**Tech Stack:** nginx, Rust (std only).

## The two defects, both measured

**1. The limiter is trivially defeated on the SPA-fronted path.** All six proxy blocks in `frontend/nginx-prod.conf` set `X-Forwarded-For $proxy_add_x_forwarded_for`, which **appends** to whatever the caller sent. The backend reads the **first** hop (`crates/egret-core/src/ratelimit.rs`'s callers do `split(',').next()`), so the caller fully controls the value the limiter keys on.

Reproduced through a port-forward to the live frontend pod on `4a5ffa37`:

```
60 × POST /oauth/token   X-Forwarded-For: 198.51.100.1  -> 401
request 61                                              -> 429
same header again                                       -> 429
X-Forwarded-For: 203.0.113.99, same client_id           -> 401   <- full fresh budget
```

**This is not only about the token endpoint.** `location /api/` carries the same header line, so the login limiter — 5 attempts / 300s, the one thing standing in front of password guessing — is defeated the same way on this deployment.

Behind the k8s ingress the limiter is sound: ingress-nginx sets `X-Forwarded-For $remote_addr` (overwrite) with `use-forwarded-headers=false`. The SPA-fronted path is the outlier, and it is the one `nginx-prod.conf` exists to serve.

**2. The limiter never forgets.** `crates/egret-core/src/ratelimit.rs` prunes each entry's `attempts` but never removes the entry itself, so the map grows with every distinct key ever seen. #526 bounded what one key **costs** (SHA-256, 32 inline bytes); it did not bound how many there are.

## Global Constraints

- **Do not weaken the limiter's semantics to make eviction easy.** An entry under an active lockout must survive: forgetting it would let a locked-out caller reset by waiting out the *window* instead of the *lockout*, which is the longer of the two by design.
- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Backend tests are DB-gated: a missing `DATABASE_URL` makes them report `ok` while asserting nothing. **Use `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test_t1"`** — the shared `egret_test` holds 3327 workspaces and `egret-auth`'s `pdp_hot_reload` fails there on timing. NEVER point tests at the dev `egret` database.
- `cargo test` without `--no-fail-fast` stops at the first failing binary.
- **A shell hook rewrites `... | grep` into `rtk grep`, which swallows piped stdin** — redirect to a file and grep the file, or you will read empty output as success.
- Run tests unpiped; NEVER pipe into `head`/`tail`.
- **Commit before cutting any mutation.** A mutation that fails to compile — or a patch that silently fails to apply because `cargo fmt` reflowed the target — proves nothing.
- A kind cluster is live. Leave `GET /kind/smoke` in `demo` `live` and non-trashed.

---

### Task 1: The edge stops trusting a caller-supplied client IP

**Files:** `frontend/nginx-prod.conf`

- [ ] **Step 1: Overwrite rather than append, in all six proxy blocks**

`$remote_addr`, not `$proxy_add_x_forwarded_for` — the same posture ingress-nginx takes by default, and for the same reason.

The blocks are `location /api/`, `/data-api/`, `/oauth/`, `= /catalog`, `/catalog/`, and the two `.well-known` exact locations. **Count them yourself and change every one**; a limiter that holds on five of six paths is a limiter with a documented bypass.

- [ ] **Step 2: Write down the trade-off you are making**

Overwriting means that if something *else* fronts this nginx — a cloud LB, a CDN — the real client IP is lost and every caller behind it shares one bucket. That is the **safe** direction (over-limiting, never under-limiting), and it is why this is the right default. Say so in a comment, and say that trusting an upstream proxy is opt-in via nginx's `real_ip` module with an explicit `set_real_ip_from` — not by going back to appending.

- [ ] **Step 3: Prove it with the real config, not by reading it**

`cspFrameSrc.test.ts` already reads this file, so a syntax error will not go unnoticed — but it cannot tell you what nginx *does*. Run the real config under `nginx:alpine` with the upstream absent (a 502 proves the request was proxied rather than served from the static root), and confirm the header nginx forwards is `$remote_addr` regardless of what the client sent. The previous task in this area used exactly this technique; reuse it.

- [ ] **Step 4: Commit**

---

### Task 2: The limiter forgets callers it is no longer tracking

**Files:** `crates/egret-core/src/ratelimit.rs`

- [ ] **Step 1: Drop entries that are tracking nothing**

After pruning an entry's `attempts`, an entry with no attempts left in the window **and** no active lockout is holding no information — remove it. Do this as part of the existing pass rather than adding a background task; the limiter is a `Mutex<HashMap>` and a sweep is not free, so say in a comment what your approach costs and why you chose it.

- [ ] **Step 2: Write the tests, and make the third one the one that bites**

1. An entry with no attempts in-window and no lockout is gone after a later check.
2. The map does not grow without bound across many distinct keys whose windows have passed.
3. **An entry under an active lockout SURVIVES.** This is the assertion that separates a correct eviction from one that hands a locked-out caller a fresh budget early — the exact thing an attacker wants. A test that only checks the map shrinks would pass against that bug.

You will need to observe the map's size. Add the smallest accessor that makes the test possible and say why it exists.

Use the injected-instant idiom already in this file (`check_at`) rather than sleeping.

- [ ] **Step 3: Run, mutation-check, commit**

Mutations: remove the eviction (tests 1-2 fail); evict unconditionally, ignoring `locked_until` (test 3 fails). Build to zero errors and confirm the patch landed before reading either result.

---

## After the plan

Rebuild and redeploy to kind, then **re-run the exact reproduction above** — the same 61 requests with a fixed spoofed `X-Forwarded-For`, then one header edit. The final request must now return **429**, not 401.

Preserve the release's values (`helm get values egret -n egret-system`), and verify the roll by **pod AGE, not the rollout message** — images are tagged `:dev`, so helm reports success while old pods keep running.

## Self-Review

**Spec coverage:** the two measured defects map to Tasks 1 and 2. **Placeholder scan:** Task 2's tests are described rather than written because the accessor they need does not exist yet; the falsifiability requirement is stated instead. **Type consistency:** `check`/`check_at` keep their signatures; only the internal map handling changes.
