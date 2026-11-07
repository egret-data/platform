"""POST a statement to Trino, follow nextUri, print 'HTTP <first status> | <rows or error>'.
usage: query.py BASE SQL [-u user:pw] [-H 'Name: value']...   (TLS verification is off: self-signed)"""
import base64, json, ssl, sys, urllib.error, urllib.parse, urllib.request

base, sql, *rest = sys.argv[1:]
hdrs = {}
i = 0
while i < len(rest):
    if rest[i] == "-u":
        hdrs["Authorization"] = "Basic " + base64.b64encode(rest[i + 1].encode()).decode()
    elif rest[i] == "-H":
        k, v = rest[i + 1].split(": ", 1)
        hdrs[k] = v
    i += 2
ctx = ssl.create_default_context()
ctx.check_hostname = False
ctx.verify_mode = ssl.CERT_NONE


def call(url, data=None):
    req = urllib.request.Request(url, data=data, headers=hdrs)
    try:
        with urllib.request.urlopen(req, context=ctx, timeout=30) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


status, body = call(base + "/v1/statement", sql.encode())
first = status
rows, err = [], None
while status == 200:
    j = json.loads(body)
    rows += j.get("data") or []
    err = (j.get("error") or {}).get("message") or err
    if not j.get("nextUri"):
        break
    # behind X-Forwarded-Proto: https Trino advertises https nextUri; stay on `base`
    u = urllib.parse.urlparse(j["nextUri"])
    status, body = call(base + u.path + ("?" + u.query if u.query else ""))
if first != 200:
    print(f"HTTP {first} | {' '.join(body.split())[:200]}")
else:
    print(f"HTTP {first} | {err or json.dumps(rows)}")
