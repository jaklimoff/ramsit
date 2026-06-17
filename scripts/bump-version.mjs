// Sync the release version across the JS and Rust manifests.
// Invoked by semantic-release (@semantic-release/exec) as:
//   node scripts/bump-version.mjs <version>
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2];
if (!version) {
  console.error("usage: node scripts/bump-version.mjs <version>");
  process.exit(1);
}

const setJsonVersion = (path) => {
  const json = JSON.parse(readFileSync(path, "utf8"));
  json.version = version;
  writeFileSync(path, JSON.stringify(json, null, 2) + "\n");
};

setJsonVersion("package.json");
setJsonVersion("src-tauri/tauri.conf.json");

// Cargo.toml: the version line directly under the [package] `name = "ramsit"`.
const cargoToml = "src-tauri/Cargo.toml";
writeFileSync(
  cargoToml,
  readFileSync(cargoToml, "utf8").replace(
    /(name = "ramsit"\nversion = ")[^"]*(")/,
    `$1${version}$2`,
  ),
);

// Cargo.lock: the [[package]] entry for ramsit.
const cargoLock = "src-tauri/Cargo.lock";
writeFileSync(
  cargoLock,
  readFileSync(cargoLock, "utf8").replace(
    /(name = "ramsit"\nversion = ")[^"]*(")/,
    `$1${version}$2`,
  ),
);

console.log(`bumped to ${version}`);
