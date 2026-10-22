// Values the release artifacts must agree on.
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { REPO } from "./server";

export const VERSION = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(join(REPO, "Cargo.toml"), "utf8"))![1];
