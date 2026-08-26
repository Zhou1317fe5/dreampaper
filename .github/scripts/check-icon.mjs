import { readFileSync } from "node:fs";

const source = readFileSync("static/favor.ico");
const bundled = readFileSync("src-tauri/icons/icon.ico");

if (!source.equals(bundled)) {
  console.error("src-tauri/icons/icon.ico 与 static/favor.ico 不一致，请先同步图标。");
  process.exit(1);
}

console.log("Windows 打包图标与 static/favor.ico 一致");
