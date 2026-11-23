// package.json is the single source of truth for the pi distro version: the
// compiled binary cannot read its own package.json, so src/version.ts mirrors
// the pin (checked here), and the desktop app reads package.json itself at
// compile time (crates/steer-app/build.rs, STEER_PI_DISTRO_VERSION) — there is
// no app-side mirror constant to keep in step.
import { readFileSync } from "node:fs";

const pkg = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const pin = pkg.dependencies["@earendil-works/pi-coding-agent"];
const source = readFileSync(new URL("../src/version.ts", import.meta.url), "utf8");
if (!source.includes(`"${pin}"`)) {
  console.error(`src/version.ts must carry the pinned version ${pin}`);
  process.exit(1);
}
