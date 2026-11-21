// Size budget for the script-tag bundle (docs/decisions.md, D-003).
import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";
const BUDGET = 10 * 1024;
const file = new URL("../dist/openlens.min.js", import.meta.url);
const size = gzipSync(readFileSync(file), { level: 9 }).length;
console.log(`openlens.min.js: ${size} bytes gzipped (budget ${BUDGET})`);
if (size > BUDGET) {
  console.error("over budget");
  process.exit(1);
}
