import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2]?.trim();

if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version ?? "")) {
  console.error(
    `版本号 "${version ?? ""}" 不是合法 semver。tag 需形如 v1.2.3 或 v1.2.3-rc.1。`
  );
  process.exit(1);
}

function patch(path, pattern) {
  const original = readFileSync(path, "utf8");

  if (!pattern.test(original)) {
    console.error(`${path}: 未匹配到版本号字段，格式可能已变，请检查脚本。`);
    process.exit(1);
  }

  const updated = original.replace(pattern, `$1"${version}"`);
  if (updated !== original) writeFileSync(path, updated);
  console.log(`${path} → ${version}${updated === original ? "（本来就是该版本，未改动）" : ""}`);
}

const topLevelVersion = /^(\s{2}"version":\s*)"[^"]*"/m;

patch("src-tauri/tauri.conf.json", topLevelVersion);

patch("src-tauri/Cargo.toml", /^(\[package\][\s\S]*?^version\s*=\s*)"[^"]*"/m);

patch("package.json", topLevelVersion);

