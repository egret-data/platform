"""E2E ONLY (port-8080 hardening, A5). Used by up.sh step 7b.

    python3 -I copy_auth.py <trino-ranger config.properties> <trino-opa config.properties>

Copies trino-opa's authentication lines (process-forwarded, http-server.authentication.*,
http-server.https.*, internal-communication.shared-secret) into trino-ranger's config.properties,
between BEGIN/END markers that a re-run replaces. The shared secret is egret's `${ENV:...}`
reference on trino-opa; trino-ranger gets a fresh secret of its own instead.
"""

import re
import secrets
import sys

BEGIN = "# BEGIN egret-opa-e2e authentication (copied from trino-opa)"
END = "# END egret-opa-e2e authentication"
KEEP = re.compile(
    r"^(http-server\.process-forwarded=|http-server\.authentication\.|http-server\.https\."
    r"|internal-communication\.shared-secret=)"
)
SECRET = "internal-communication.shared-secret="


def merged(ranger: str, opa: str, secret: str) -> str:
    lines = [l for l in opa.splitlines() if KEEP.match(l)]
    lines = [SECRET + secret if l.startswith(SECRET) else l for l in lines]
    body = re.sub(re.escape(BEGIN) + r".*?" + re.escape(END) + r"\n?", "", ranger, flags=re.S)
    return body.rstrip("\n") + "\n" + "\n".join([BEGIN, *lines, END]) + "\n"


def main() -> None:
    cfg, opa = sys.argv[1:]
    with open(cfg) as f:
        ranger = f.read()
    with open(opa) as f:
        opa_body = f.read()
    with open(cfg, "w") as f:
        f.write(merged(ranger, opa_body, secrets.token_hex(32)))


if __name__ == "__main__":
    main()
