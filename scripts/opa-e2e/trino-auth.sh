# E2E ONLY (port-8080 hardening, A5). Sourced (bash) by up.sh, layer2.sh, live-check.sh and
# latency.sh after they set REPO and COMPOSE. How the harness authenticates to both engines.
#
# Every authenticated engine egret renders refuses authentication over plain HTTP
# (`http-server.authentication.allow-insecure-over-http=false`): a request on 8080 without
# `X-Forwarded-Proto: https` is answered 403 before any identity is read, and one with that header
# must carry a credential (401 otherwise). See docs/en/plans/2026-11-07-port-8080-hardening-live-result.md.
# So the harness does what egret's own query path does: it authenticates as the delegator
# (`egret_delegator`, Basic) and names the session user with `X-Trino-User`
# (`X-Trino-Original-User: egret_delegator`), which the profile's infrastructure policy lets the
# delegator impersonate. trino-ranger is given the same authentication by up.sh (step 7b).
#
# The delegator's password: egret never returns it over its API (no reveal route). egret stores it
# sealed (AES-256-GCM, egret_secrets::crypto) in this stack's Postgres, under the engine's
# `delegator_secret_id`, with the PUBLIC development master key in deploy/opa-e2e/egret.env. The
# harness reads that row and opens it. AES-GCM encrypts with AES-CTR from counter block
# nonce || 00000002, so `openssl enc -aes-256-ctr` with that IV yields the plaintext of the
# ciphertext without its 16-byte tag. The tag is not checked here; the identity check's positive
# control (the delegator authenticates and becomes bob) is what proves the password.

DELEGATOR=${DELEGATOR:-egret_delegator} # egret_core::engine_principals::DELEGATOR_PRINCIPAL

# delegator_password ENGINE_NAME: the plaintext delegator password of the egret engine, or fail.
delegator_password() {
  local row key
  row=$("${COMPOSE[@]}" exec -T postgres psql -U egret -d egret -At -F ' ' -c \
    "SELECT encode(sd.nonce, 'hex'), encode(sd.ciphertext, 'hex')
       FROM engine_mgmt.engines e
       JOIN workspace_secrets ws ON ws.id = e.delegator_secret_id AND ws.workspace_id IS NULL
       JOIN secret_data sd ON sd.secret_id = ws.secret_ref AND sd.key = 'value'
      WHERE e.name = '$1'") || return 1
  [ -n "$row" ] || { printf 'trino-auth.sh: engine %s has no delegator secret\n' "$1" >&2; return 1; }
  key=$(sed -n 's/^EGRET_SECRETS__MASTER_KEY=//p' "$REPO/deploy/opa-e2e/egret.env")
  [ -n "$key" ] || { printf 'trino-auth.sh: no EGRET_SECRETS__MASTER_KEY in egret.env\n' >&2; return 1; }
  python3 -I - "$row" "$key" <<'PY'
import base64, subprocess, sys
nonce_hex, ct_hex = sys.argv[1].split(" ")
nonce, ct, key = bytes.fromhex(nonce_hex), bytes.fromhex(ct_hex), base64.b64decode(sys.argv[2])
if len(nonce) != 12 or len(ct) <= 16 or len(key) < 32:
    sys.exit("trino-auth.sh: the sealed delegator secret has an unexpected shape")
iv = nonce + (2).to_bytes(4, "big")  # GCM's first counter block for a 96-bit nonce
p = subprocess.run(["openssl", "enc", "-d", "-aes-256-ctr", "-nopad", "-K", key[:32].hex(), "-iv", iv.hex()],
                   input=ct[:-16], capture_output=True, check=True)
sys.stdout.write(p.stdout.decode("utf-8"))
PY
}

# trino_cli CONTAINER USER SQL [cli args...]: the Trino CLI inside CONTAINER, over HTTPS 8443 (the
# CLI refuses a password over HTTP), as the delegator impersonating USER. TRINO_DELEGATOR_PASSWORD
# must be set; it reaches the CLI through the environment, never through argv.
trino_cli() {
  local c=$1 u=$2 sql=$3
  shift 3
  TRINO_PASSWORD="$TRINO_DELEGATOR_PASSWORD" docker exec -e TRINO_PASSWORD "$c" trino \
    --server https://127.0.0.1:8443 --insecure --user "$DELEGATOR" --password --session-user "$u" \
    --output-format TSV "$@" --execute "$sql"
}

# identity_check CONTAINER LABEL: scripts/opa-e2e/identity-check.sh inside CONTAINER; fails the
# caller's run on any failed check.
identity_check() {
  docker exec -e TRINO_DELEGATOR_PASSWORD "$1" sh -c "$(cat "$REPO/scripts/opa-e2e/identity-check.sh")" \
    identity-check "$2"
}
