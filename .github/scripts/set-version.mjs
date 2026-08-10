// 把 tag 版本写进包内版本号。
//
// 不做这一步的话，安装包文件名带着 tag 版本、而「关于」里显示的还是仓库里
// 硬编码的 0.1.0，用户与我们都无法从已安装的应用判断它是哪个版本。
//
// 用法：node .github/scripts/set-version.mjs 1.2.3

import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2]?.trim();

// Tauri 会把这个值交给 NSIS / dmg，非 semver 会在打包中途才炸，
// 与其那样不如在这里立刻失败并说清原因。
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version ?? "")) {
  console.error(
    `版本号 "${version ?? ""}" 不是合法 semver。tag 需形如 v1.2.3 或 v1.2.3-rc.1。`
  );
  process.exit(1);
}

// 分两步判断，不能只看「内容有没有变」：重打同一个版本（或首发 tag 恰好
// 等于仓库里硬编码的版本）时替换本就是空操作，把它当成失败会让整条流水线
// 在编译前就挂掉，而真正的格式漂移反而查不出来。
// 所以：匹配不上才是错，匹配上但值没变属正常。
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

// 全部用定点替换而非 JSON 重新序列化：后者会把 "targets": ["app","dmg","nsis"]
// 这类紧凑写法展开成多行，让 CI 的 diff 里混进一堆与版本无关的改动。
// 顶层 "version" 固定是两格缩进，不会误伤嵌套字段。
const topLevelVersion = /^(\s{2}"version":\s*)"[^"]*"/m;

patch("src-tauri/tauri.conf.json", topLevelVersion);

// 只替换 [package] 段里的第一个 version，不能碰依赖项的版本号
patch("src-tauri/Cargo.toml", /^(\[package\][\s\S]*?^version\s*=\s*)"[^"]*"/m);

patch("package.json", topLevelVersion);

