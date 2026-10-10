// Records WHICH spec the committed orval client was emitted from (SP4d Task 10).
//
// `egret-server`'s `committed_openapi_json_is_current` compares the committed
// `openapi.json` against a FRESH emit, so the spec cannot drift from the Rust
// types. Nothing compared the spec against `frontend/src/api/generated/`, and
// the hole is not hypothetical: `3d8ee22a` changed `openapi.json` without
// running `npm run gen:api`, the drift survived two tasks, and a third task's
// own regeneration caught up two doc sweeps at once.
//
// So `npm run gen:api` now writes a two-line receipt beside the client:
//
//   spec   — sha256 of the `openapi.json` bytes orval read
//   client — sha256 over the emitted tree (see `treeDigest` for the exact
//            pre-image, which `egret-server`'s gate recomputes byte-for-byte)
//
// `generated_client_was_emitted_from_the_committed_spec` in
// `crates/egret-server/src/lib.rs` recomputes both and fails if either moved.
// It needs no network and no `node` — which is the point, because `just check`
// is this project's only gate (GitHub CI is out of quota) and must stay
// runnable on a machine with no npm install.
//
// # What this proves, and what it does not
//
// It proves the two artefacts were last written TOGETHER by this script, and
// that neither has been touched since. It is a chain-of-custody receipt, not an
// independent regeneration: this script is reachable on its own, so somebody who
// edits a generated file and then runs `node scripts/write-api-digest.mjs`
// instead of `npm run gen:api` re-blesses their edit. The cheap gate catches the
// forgetful case — which is the one that has actually happened here — and an
// independent check would have to run orval, which needs the network on a cold
// `node_modules` and would put ~10 s of codegen inside `just check`.
//
// Only `.ts` files are digested. Orval emits nothing else (measured: 525 of 525
// files are `.ts`), and hashing every file would turn a stray `.DS_Store` into a
// red gate — a guard that cries wolf gets deleted. A non-`.ts` file added under
// `generated/` is therefore outside this receipt's reach.
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync, statSync } from "node:fs";
import { join } from "node:path";

const FRONTEND = new URL("..", import.meta.url).pathname;
const SPEC = join(FRONTEND, "..", "openapi.json");
const GENERATED = join(FRONTEND, "src", "api", "generated");
const DIGEST = join(FRONTEND, "src", "api", "generated.digest.json");

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Every `.ts` file under `dir`, as paths relative to it, `/`-separated. */
function tsFiles(dir, prefix = "") {
  const out = [];
  for (const entry of readdirSync(dir)) {
    const abs = join(dir, entry);
    const rel = prefix ? `${prefix}/${entry}` : entry;
    if (statSync(abs).isDirectory()) {
      out.push(...tsFiles(abs, rel));
    } else if (rel.endsWith(".ts")) {
      out.push(rel);
    }
  }
  return out;
}

/**
 * The tree digest's pre-image, stated here because a second implementation
 * (`crates/egret-server/src/lib.rs`) has to reproduce it exactly: one
 * `<relative path> <sha256 of the file>\n` line per file, paths sorted by their
 * bytes. Paths are IN the pre-image, so renaming a file moves the digest even
 * when its contents are unchanged.
 */
function treeDigest(dir) {
  const files = tsFiles(dir).sort();
  let preimage = "";
  for (const rel of files) {
    preimage += `${rel} ${sha256(readFileSync(join(dir, rel)))}\n`;
  }
  return { digest: sha256(preimage), files: files.length };
}

const { digest: client, files } = treeDigest(GENERATED);
const receipt = {
  _comment:
    "Written by `npm run gen:api`; checked by egret-server's " +
    "generated_client_was_emitted_from_the_committed_spec. Do not hand-edit — " +
    "regenerate instead. See frontend/scripts/write-api-digest.mjs.",
  spec: sha256(readFileSync(SPEC)),
  client,
  files,
};
writeFileSync(DIGEST, `${JSON.stringify(receipt, null, 2)}\n`);
console.log(
  `write-api-digest: spec ${receipt.spec.slice(0, 12)}… client ${client.slice(0, 12)}… over ${files} file(s)`,
);
