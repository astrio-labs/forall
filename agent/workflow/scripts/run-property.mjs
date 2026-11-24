#!/usr/bin/env node
/**
 * Invoked by forall-workflow property-tests phase.
 * Loads a `.property.ts` default export and prints structured JSON to stdout.
 */
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

// Node reports an unresolved package as text and exposes no field for the
// specifier, so the name and the importer are read back out of the message.
const PACKAGE_NOT_FOUND = /^Cannot find package '([^']+)' imported from (.+)$/;

/**
 * Whether any manifest at or above `fromDir` asks for `name`.
 *
 * This is what separates "not installed" from "not a real package". A
 * dependency somebody declared and did not install is an environment gap; a
 * name nobody declared is one the author got wrong. Walking up also covers
 * the generator library's own dependencies, because an importer inside
 * node_modules reaches that library's manifest first.
 */
function isDeclaredDependency(name, fromDir) {
  const fields = [
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
  ];
  let dir = fromDir;
  for (;;) {
    try {
      const manifest = JSON.parse(
        fs.readFileSync(path.join(dir, "package.json"), "utf8"),
      );
      if (fields.some((field) => Object.hasOwn(manifest?.[field] ?? {}, name))) {
        return true;
      }
    } catch {
      // No manifest here, or an unreadable one. Keep walking.
    }
    const parent = path.dirname(dir);
    if (parent === dir) {
      return false;
    }
    dir = parent;
  }
}

/**
 * Whether a module load failure means the check could not run, as opposed to
 * the property module being broken.
 */
function isToolingGap(err) {
  // A *file* that is not there: a typo in a relative import, or a property
  // module that has been moved away. Node sets `url` only in that case.
  if (err?.code !== "ERR_MODULE_NOT_FOUND" || err.url !== undefined) {
    return false;
  }
  const match = PACKAGE_NOT_FOUND.exec(String(err.message ?? "").split("\n")[0]);
  if (!match) {
    // The message did not parse, so which of the two this is cannot be
    // established. A spurious failure is loud and gets fixed; a spurious
    // warning hides a property that never ran.
    return false;
  }
  const [, name, importer] = match;
  return isDeclaredDependency(name, path.dirname(importer));
}

const propertyPath = process.argv[2];
if (!propertyPath) {
  process.stdout.write(
    JSON.stringify({ ok: false, message: "missing property file path" }),
  );
  process.exit(1);
}

// Loading is separated from running on purpose. A property module that cannot
// resolve its generator library has not falsified anything — the check never
// happened — and reporting that as a failed property writes a false statement
// into the ledger.
//
// ERR_MODULE_NOT_FOUND alone is too coarse a signal: it covers a package that
// is not installed, a package that does not exist, and a file that is not
// there. Only the first is a tooling gap. The other two are defects in the
// author's own code, and downgrading those to a warning would let a property
// that can never run sit quietly under a passing check forever. See
// `isToolingGap`.
let mod;
try {
  mod = await import(pathToFileURL(path.resolve(propertyPath)).href);
} catch (err) {
  process.stdout.write(
    JSON.stringify({
      ok: false,
      toolUnavailable: isToolingGap(err),
      message: err instanceof Error ? err.message : String(err),
    }),
  );
  process.exit(1);
}

try {
  const run = mod.default;
  if (typeof run !== "function") {
    process.stdout.write(
      JSON.stringify({
        ok: false,
        message: "property module must default-export runPropertyTests()",
      }),
    );
    process.exit(1);
  }
  const result = await run();
  process.stdout.write(JSON.stringify(result ?? { ok: false }));
} catch (err) {
  process.stdout.write(
    JSON.stringify({
      ok: false,
      message: err instanceof Error ? err.message : String(err),
    }),
  );
  process.exit(1);
}
