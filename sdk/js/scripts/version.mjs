// The SDK reports its version with each event ($lib_version): it must be the
// package's, which the release train moves with Cargo.toml.
import { readFileSync } from "node:fs";

const { version } = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const src = readFileSync(new URL("../src/index.ts", import.meta.url), "utf8");
const found = src.match(/^const VERSION = "([^"]+)";$/m)?.[1];
if (found !== version) {
  console.error(`src/index.ts has VERSION ${found ?? "(missing)"}, package.json has ${version}`);
  process.exit(1);
}
