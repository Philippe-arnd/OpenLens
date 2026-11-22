// Prepend rrweb's MIT notice to the bundles that contain rrweb code. MIT asks
// for the copyright and permission notice in every copy; minifying strips
// comments, so we add it back after the build.
import { readFileSync, writeFileSync } from "node:fs";

const notice = readFileSync(new URL("../licenses/rrweb-LICENSE.txt", import.meta.url), "utf8").trim();
const banner = `/*! Includes rrweb 2.2.2 (https://github.com/rrweb-io/rrweb).\n${notice.replace(/\*\//g, "* /")}\n*/\n`;
for (const f of ["openlens-replay.min.js", "openlens-player.min.js", "openlens-player.min.css"]) {
  const path = new URL(`../dist/${f}`, import.meta.url);
  const body = readFileSync(path, "utf8");
  if (!body.startsWith("/*! Includes rrweb")) writeFileSync(path, banner + body);
}
