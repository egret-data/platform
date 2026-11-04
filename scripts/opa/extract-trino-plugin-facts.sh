#!/usr/bin/env bash
# Extract what Trino 483's OPA and Ranger plugins send and check, from the
# pinned image egret runs. Output files are committed as test fixtures of
# egret-perm-eval; re-run when KNOWN_ENGINE_IMAGES gains a Trino image.
set -euo pipefail
IMAGE="trinodb/trino:483@sha256:db58cc93e593a2706553745f276bb119c9810e69918be56ecde088ba7ccb0534"
OUT="${1:?usage: extract-trino-plugin-facts.sh <out-dir>}"
WORK="$(mktemp -d)"
cid=""
trap 'docker rm -f "$cid" >/dev/null 2>&1 || true; rm -rf "$WORK"' EXIT
cid="$(docker create "$IMAGE")"
docker cp "$cid:/usr/lib/trino/plugin/opa/io.trino_trino-opa-483.jar" "$WORK/opa.jar"
docker cp "$cid:/usr/lib/trino/plugin/ranger/io.trino_trino-ranger-483.jar" "$WORK/ranger.jar"
mkdir -p "$WORK/opa" "$WORK/ranger"
(cd "$WORK/opa" && unzip -q ../opa.jar 'io/trino/plugin/opa/*')
(cd "$WORK/ranger" && unzip -q ../ranger.jar 'io/trino/plugin/ranger/*')
(cd "$WORK" && shasum -a 256 opa.jar ranger.jar) > "$OUT/trino-483-plugins.sha256"

# Operation names: the string constants of the three classes that build requests.
for c in OpaAccessControl OpaBatchAccessControl OpaHighLevelClient; do
  javap -c -p -constants "$WORK/opa/io/trino/plugin/opa/$c.class"
done | grep -oE '// String [A-Z][A-Za-z]+$' | awk '{print $NF}' | LC_ALL=C sort -u \
  > "$OUT/trino-483-opa-operations.txt"

# Ranger plugin: each public check/filter/get method and the access types it names.
javap -c -p -constants "$WORK/ranger/io/trino/plugin/ranger/RangerSystemAccessControl.class" \
| awk '/^  public [^(]* (checkCan|filter|get)[A-Za-z]+\(/ {
         if (m != "") print m ":" acc
         match($0, /(checkCan|filter|get)[A-Za-z]+/); m = substr($0, RSTART, RLENGTH); acc = "" }
       /RangerTrinoAccessType\.[A-Z_]+/ {
         match($0, /RangerTrinoAccessType\.[A-Z_]+/); a = substr($0, RSTART + 22, RLENGTH - 22)
         if (index(acc, " " a) == 0) acc = acc " " a }
       END { print m ":" acc }' | LC_ALL=C sort -u > "$OUT/trino-483-ranger-checks.txt"

# Request shapes of the batch filters and of both mask paths, as committed evidence
# (open question (c)): the bytecode that builds FilterColumns and GetColumnMask requests.
{
  echo "## OpaBatchAccessControl: filterColumns and the batch request builders"
  javap -c -p -constants "$WORK/opa/io/trino/plugin/opa/OpaBatchAccessControl.class" \
    | awk '/filterColumns|lambda\$filterColumns\$0|lambda\$batchRequestBuilder\$[01]/,/^$/'
  echo "## OpaHighLevelClient and OpaAccessControl: the column-mask requests"
  for c in OpaHighLevelClient OpaAccessControl; do
    javap -c -p -constants "$WORK/opa/io/trino/plugin/opa/$c.class" \
      | awk '/getColumnMask|getColumnMasks|GetColumnMask/,/^$/'
  done
} > "$OUT/trino-483-opa-request-shapes.txt"
