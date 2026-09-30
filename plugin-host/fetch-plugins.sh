#!/usr/bin/env bash
# Downloads the Kestra plugin jars named in plugins.lock into the directory given as $1,
# and fails, naming the jar, unless each one's sha256 equals the lockfile's.
#
# A jar already present in $1 is verified, not downloaded again: the image build runs
# this script over the jars it was given, so the build re-checks every byte it ships.
# After every jar has passed, plugins.lock is copied into $1 so `EgretPluginHost describe`
# can stamp the catalog with the hash of the lockfile these jars were checked against.
#
# Usage: plugin-host/fetch-plugins.sh <dest-dir>
set -euo pipefail

dest="${1:?usage: fetch-plugins.sh <dest-dir>}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
lock="$here/plugins.lock"
repo="${MAVEN_CENTRAL:-https://repo1.maven.org/maven2}"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

mkdir -p "$dest"

# Everything in $dest ends up on the host's classpath (`plugins/*`): a jar the lockfile does not
# name is refused, never silently shipped.
for present in "$dest"/*.jar; do
  [ -e "$present" ] || continue
  name="$(basename "$present")"
  if ! awk '{ split($1, c, ":"); print c[2] "-" c[3] ".jar" }' "$lock" | grep -qxF "$name"; then
    echo "fetch-plugins: $name is in $dest but not in plugins.lock" >&2
    exit 1
  fi
done

while read -r coord want; do
  [ -z "$coord" ] && continue
  case "$coord" in \#*) continue ;; esac
  IFS=: read -r group artifact version <<<"$coord"
  jar="$dest/$artifact-$version.jar"

  if [ ! -f "$jar" ]; then
    url="$repo/${group//.//}/$artifact/$version/$artifact-$version.jar"
    echo "fetch-plugins: downloading $url" >&2
    curl -fsSL -o "$jar.part" "$url"
    mv "$jar.part" "$jar"
  fi

  got="$(sha256 "$jar")"
  if [ "$got" != "$want" ]; then
    echo "fetch-plugins: sha256 mismatch for $artifact-$version.jar ($coord): plugins.lock has $want, the jar is $got" >&2
    exit 1
  fi
  echo "fetch-plugins: ok $artifact-$version.jar $got" >&2
done <"$lock"

cp "$lock" "$dest/plugins.lock"
