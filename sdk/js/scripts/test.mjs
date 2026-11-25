// `node --test` on the SDK's DOM-free modules (#433): esbuild (already a
// dev dependency) bundles each test to a temporary file first.
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const dir = fileURLToPath(new URL("../test/", import.meta.url));
const out = mkdtempSync(join(tmpdir(), "openlens-sdk-test-"));
const files = [];
for (const f of readdirSync(dir).filter((f) => f.endsWith(".test.ts"))) {
  const outfile = join(out, f.replace(/\.ts$/, ".mjs"));
  await build({ entryPoints: [join(dir, f)], bundle: true, platform: "node", format: "esm", outfile, logLevel: "error" });
  files.push(outfile);
}
const r = spawnSync(process.execPath, ["--test", ...files], { stdio: "inherit" });
process.exit(r.status ?? 1);
