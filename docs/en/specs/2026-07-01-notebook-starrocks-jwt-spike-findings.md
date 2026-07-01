# Spike (P0): Python / Daft → StarRocks authenticating with a per-user OIDC JWT

- **Date:** 2026-07-01
- **Status:** DONE — question answered **YES**, direct transport is viable.
- **Question:** Can a Python/Daft process connect to StarRocks authenticating with a
  per-user OIDC JWT (instead of routing the query through the Rust backend), so a future
  `starrocks(sql)` notebook helper can run the query in-kernel?
- **Answer:** **Yes, directly.** `daft.read_sql(...)` works over a SQLAlchemy/PyMySQL
  connection using a ~15-line custom auth-plugin handler that speaks StarRocks'
  `authentication_openid_connect_client` auth-switch. Verified end-to-end against the live
  dev stack.

---

## 1. Environment

| Component | Value |
|---|---|
| StarRocks version | `4.1.1-14b7e3f` (`SELECT current_version();`) |
| StarRocks FE | `egret-dev-starrocks-1`, MySQL protocol `127.0.0.1:9030`, http `:8030` |
| Keycloak | `egret-dev-keycloak-1` (Keycloak 26), host `:8081`, realm `egret` |
| Client python | 3.12 (scratch `uv venv`); `daft 0.7.16`, `pymysql 1.2.0`, `sqlalchemy 2.0.51`, `mysql-connector-python 9.7.0`, `sqlglot` |

### JWT security integration — already present and accepted

The realm and StarRocks were already provisioned. The integration is accepted and active
in the authentication chain (`authentication_chain = [native, egret_oidc]`):

```sql
SHOW CREATE SECURITY INTEGRATION egret_oidc;
```

```text
CREATE SECURITY INTEGRATION `egret_oidc` PROPERTIES (
  "type"              = "authentication_jwt",
  "required_issuer"   = "http://keycloak:8081/realms/egret",
  "required_audience" = "starrocks",
  "principal_field"   = "preferred_username",
  "jwks_url"          = "http://keycloak:8080/realms/egret/protocol/openid-connect/certs"
)
```

The JWT-mapped user also exists (this is what actually authenticated in the spike):

```text
'tester'@'%'  AUTHENTICATION_JWT  {
  "jwks_url":          "http://keycloak:8080/realms/egret/protocol/openid-connect/certs",
  "principal_field":   "preferred_username",
  "required_issuer":   "http://localhost:8081/realms/egret",   -- matches token iss
  "required_audience": "starrocks"
}
```

> Note: `jwks_url` uses the in-Docker host `keycloak:8080` (reachable from the SR container),
> while `required_issuer` must equal the **token's** `iss` = `http://localhost:8081/realms/egret`
> (what host clients mint against). The per-user plugin config already has the correct issuer;
> the integration's `required_issuer` (`keycloak:8081`) differs but is not on the path that
> authenticated the explicit user. For a fresh setup, set both `required_issuer` values to the
> issuer the tokens actually carry.

---

## 2. Minting the per-user JWT

```bash
curl -s -X POST "http://localhost:8081/realms/egret/protocol/openid-connect/token" \
  -d "grant_type=password" \
  -d "client_id=egret" -d "client_secret=egret-secret" \
  -d "username=tester@egret.local" -d "password=pw" \
  -d "scope=openid" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['access_token'])"
```

Decoded access-token claims (the `aud-starrocks` mapper on the `egret` client injects the
`starrocks` audience automatically — no extra scope needed):

```json
{
  "iss": "http://localhost:8081/realms/egret",
  "aud": ["egret-catalog", "starrocks", "trino"],
  "azp": "egret",
  "typ": "Bearer",
  "preferred_username": "tester",
  "email": "tester@egret.local",
  "scope": "openid email profile"
}
```

`aud` contains `starrocks` and `preferred_username = tester` matches the SR user. Good.

---

## 3. The wire protocol (why off-the-shelf drivers fail)

StarRocks does **not** use `mysql_clear_password` for JWT users. For an `AUTHENTICATION_JWT`
user the FE sends an **auth-switch request** naming plugin
`authentication_openid_connect_client`, and expects the client to reply with a single packet:

```text
[0x01][lenenc(len(JWT))][JWT bytes]     # int1 tag, then a length-encoded string; NO NUL terminator
```

This is exactly what the repo's patched `mysql_async` does
(`third_party/mysql_async/src/conn/mod.rs`, `openid_auth_data()` / `OPENID_CONNECT_CLIENT_PLUGIN`).
The `enable_cleartext_plugin(true)` in `crates/egret-engine/src/connector/starrocks.rs` is a
red herring: the actual send goes through the OpenID branch, not the cleartext branch.

Because this is a StarRocks-/Oracle-specific plugin, stock MySQL clients that don't ship it
(or that gate it on TLS) cannot log in unmodified.

---

## 4. Transport results

### A — Daft via SQLAlchemy MySQL connection factory

| Driver / approach | Result |
|---|---|
| **A0. Plain PyMySQL** (JWT as `password`) | ❌ `(2059, "Authentication plugin 'authentication_openid_connect_client' not configured")` — PyMySQL has no built-in handler for this plugin. |
| **A0b. Plain mysql-connector-python 9.7** (JWT as `password`) | ❌ `2000 (HY000) Unknown MySQL error` (it prints "The path to ID token file is not set" — it *has* an OpenID plugin but wants a token file, see A3). |
| **A1. PyMySQL + custom auth-plugin handler** | ✅ **SUCCESS.** `SELECT current_user()` → `'tester'@'%'`, `current_version()` → `4.1.1-14b7e3f`. |
| **A2. `daft.read_sql("SELECT 1 …", conn_factory)`** over SQLAlchemy engine whose `creator` is the PyMySQL+handler connection | ✅ **SUCCESS.** Returned a Daft frame with `u = 'tester'@'%'` (needed `pip install sqlglot`, a Daft `read_sql` dependency). |
| **A3. mysql-connector-python 9.7 built-in OpenID plugin** (`openid_token_file=<path>`) | ⚠️ Refuses our dev server: *"The client-server connection is insecure. Please make sure either a TLS, socket or shared memory connection is established"* → `2000`. The official plugin **mandates TLS/socket**; dev StarRocks `:9030` is plaintext. Would work against a TLS-enabled FE. |

The A1/A2 winning recipe (this is the full handshake needed):

```python
import struct
import daft
from sqlalchemy import create_engine
import pymysql

OPENID_PLUGIN = "authentication_openid_connect_client"

def _lenenc(n: int) -> bytes:
    if n < 0xFB:        return bytes([n])
    if n < 0x1_0000:    return b"\xfc" + struct.pack("<H", n)
    if n < 0x100_0000:  return b"\xfd" + struct.pack("<I", n)[:3]
    return b"\xfe" + struct.pack("<Q", n)

def _openid_auth_data(token: str) -> bytes:
    t = token.encode()
    return b"\x01" + _lenenc(len(t)) + t          # [0x01][lenenc][JWT], no NUL

class OpenIDConnectHandler:                        # PyMySQL custom auth plugin
    def __init__(self, con): self._con = con
    def authenticate(self, pkt):
        self._con.write_packet(_openid_auth_data(JWT))
        reply = self._con._read_packet(); reply.check_error()
        return reply

def _connect():
    return pymysql.connect(
        host="127.0.0.1", port=9030, user="tester", password="",
        auth_plugin_map={OPENID_PLUGIN: OpenIDConnectHandler},
    )

engine = create_engine("mysql+pymysql://", creator=_connect)
df = daft.read_sql("SELECT 1 AS n, current_user() AS u", lambda: engine.connect())
df.collect()   # -> n=1, u='tester'@'%'
```

### B — Raw `mysql` client 9.2+ with `--authentication-openid-connect-client-id-token-file`

Not executed as a positive login (no MySQL 9.2+ client available locally). Two data points:

- The `mysql 8.0.45` client shipped **inside** the SR container fails at the switch:
  `ERROR 2059 (HY000): Authentication plugin 'authentication_openid_connect_client' cannot be
  loaded: …/authentication_openid_connect_client.so: cannot open shared object file` — the
  8.0.x client simply lacks the plugin `.so`.
- Server-side JWT acceptance is nonetheless **proven** by A1 above (a hand-rolled client
  completed the exact same handshake and authenticated as the JWT principal). A real MySQL
  9.2+ client (which ships the plugin) would present the same
  `[0x01][lenenc(JWT)]` frame and succeed — likely also gated on TLS, like A3.

### C — Why the Trino-style bearer approach does not apply

StarRocks speaks the **MySQL wire protocol** on `:9030`, not HTTP. The Trino connector
authenticates by sending an `Authorization: Bearer <jwt>` HTTP header (`JWTAuthentication`).
There is no HTTP request in the StarRocks query path to attach a bearer header to; the JWT
must instead be delivered *inside the MySQL handshake* as the OpenID auth-switch payload
described in §3. So the Trino bearer transport is structurally inapplicable here.

---

## 5. RECOMMENDATION — `starrocks()` helper transport: **DIRECT**

Build the `starrocks(sql)` notebook helper as a **direct in-kernel `daft.read_sql`** using a
PyMySQL connection with the small custom `authentication_openid_connect_client` handler from
§4 (the per-user JWT is minted/downscoped by the F2 OIDC-delegation layer and injected into
the kernel). This mirrors the Iceberg/native-Daft path and keeps result materialization off
the Rust backend.

Rationale / caveats:

- **Proven working today** end-to-end (`daft.read_sql` → JWT-authenticated frame) against the
  live dev stack — no server changes required; the JWT integration already exists.
- The handler is ~15 lines and depends only on PyMySQL's public `auth_plugin_map` extension
  point (no monkey-patching, no forked driver — unlike the Rust side which needed a patched
  `mysql_async`). Bundle it in `egret-kernel-helpers`.
- Prefer PyMySQL+handler over the official `mysql-connector-python` OpenID plugin: the latter
  **refuses non-TLS connections** (A3), so it can't hit the current plaintext dev FE. When the
  StarRocks FE gets TLS in prod, either driver works; the PyMySQL handler works in both.
- Add `sqlglot` to the kernel image (Daft `read_sql` needs it for schema inference).

**Fallback (hybrid)** — only if a future security policy forbids handing the raw StarRocks JWT
to the kernel process: keep routing StarRocks queries through the existing Rust
`POST /kernel/query` callback (the backend already holds the patched `mysql_async` path). This
is a policy choice, not a technical blocker — direct is technically unblocked now.
