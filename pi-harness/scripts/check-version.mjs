// The compiled binary cannot read its own package.json, so the version is a
// source constant; this keeps it equal to the pin. A packaged app shows the
// same number in its About page, so the app-side mirror must move with these
// two (PLAN-ABSORB-06-PI-SHEET-UI adds that leg when the surface lands).
import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const pin = pkg.dependencies["@earendil-works/pi-coding-agent"];
const source = readFileSync(new URL("../src/version.ts", import.meta.url), "utf8");
if (!source.includes(`"${pin}"`)) {
  console.error(`src/version.ts must carry the pinned version ${pin}`);
  process.exit(1);
}
