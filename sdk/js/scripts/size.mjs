// Size budget for the script-tag bundle (docs/decisions.md, D-003).
// With `--badge <file>`, also writes the measured size as a shields.io
// endpoint JSON (#302): Pages publishes it for the README badge.
import { readFileSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
const BUDGET = 10 * 1024;
const file = new URL("../dist/openlens.min.js", import.meta.url);
const size = gzipSync(readFileSync(file), { level: 9 }).length;
console.log(`openlens.min.js: ${size} bytes gzipped (budget ${BUDGET})`);
const i = process.argv.indexOf("--badge");
if (i > 0 && process.argv[i + 1]) {
  const kb = (size / 1000).toFixed(1);
  writeFileSync(
    process.argv[i + 1],
    JSON.stringify({ schemaVersion: 1, label: "SDK", message: `${kb} kB gz`, color: size > BUDGET ? "red" : "0e9f6e" }),
  );
}
if (size > BUDGET) {
  console.error("over budget");
  process.exit(1);
}
