// 构建并暂存 OCR 辅助进程与 ONNX Runtime，供 `tauri build` / `tauri dev` 打包。
//
// 产物位置（均被 .gitignore 忽略）：
//   src-tauri/binaries/dreampaper-ocr-<triple>[.exe]   Tauri externalBin 约定命名
//   src-tauri/runtime/ort/<libonnxruntime.dylib|onnxruntime.dll>  平台配置引用的运行时
//   src-tauri/runtime/ort/manifest.json                本次暂存的版本、字节数与 SHA-256
//
// 运行时来源按顺序查找：
//   1. src-tauri/runtime/<triple>/<lib>（CI 或开发者放置的自构建产物）
//   2. src-tauri/runtime/runtime.json 中该 triple 的 url + sha256（下载后校验）
//   3. 仅 macOS：src-tauri/target/gate/ort-build-<arch>/Release/<lib>（本机 gate 构建）
// 找不到即失败：安装包绝不能缺少运行时而静默发布。

import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readFileSync, realpathSync, statSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';

const root = resolve(dirname(new URL(import.meta.url).pathname), '..');
const args = process.argv.slice(2);
const explicit = args.indexOf('--target');
const triple = explicit >= 0 ? args[explicit + 1] : execFileSync('rustc', ['--print', 'host-tuple']).toString().trim();
if (!triple) throw new Error('无法确定目标 triple');
const host = execFileSync('rustc', ['--print', 'host-tuple']).toString().trim();
const windows = triple.includes('windows');
const darwin = triple.includes('apple-darwin');
const exe = windows ? '.exe' : '';
const lib = darwin ? 'libonnxruntime.dylib' : windows ? 'onnxruntime.dll' : 'libonnxruntime.so';
const arch = triple.split('-')[0];

// 1. 构建辅助进程（release，engine 特性）。
const cargoArgs = ['build', '--release', '--manifest-path', resolve(root, 'src-tauri/ocr/Cargo.toml'), '--bin', 'dreampaper-ocr'];
if (triple !== host) cargoArgs.push('--target', triple);
run('cargo', cargoArgs);
const built = resolve(root, 'src-tauri/ocr/target', triple !== host ? triple : '', 'release', `dreampaper-ocr${exe}`);
if (!existsSync(built)) throw new Error(`未找到辅助进程产物：${built}`);
const binaries = resolve(root, 'src-tauri/binaries');
mkdirSync(binaries, { recursive: true });
const sidecar = resolve(binaries, `dreampaper-ocr-${triple}${exe}`);
copyFileSync(built, sidecar);

// 2. 定位运行时。
const staged = resolve(root, 'src-tauri/runtime', triple, lib);
let source = existsSync(staged) ? staged : null;
const manifestPath = resolve(root, 'src-tauri/runtime/runtime.json');
const manifest = existsSync(manifestPath) ? JSON.parse(readFileSync(manifestPath, 'utf8')) : { runtimes: {} };
const pinned = manifest.runtimes?.[triple];
if (!source && pinned?.url) {
  mkdirSync(dirname(staged), { recursive: true });
  run('curl', ['-fL', '--retry', '3', '-o', staged, pinned.url]);
  source = staged;
}
if (!source && darwin) {
  const gate = resolve(root, `src-tauri/target/gate/ort-build-${arch}/Release/${lib}`);
  if (existsSync(gate)) {
    mkdirSync(dirname(staged), { recursive: true });
    copyFileSync(realpathSync(gate), staged);
    source = staged;
  }
}
if (!source) {
  const message = `缺少 ${triple} 的 ONNX Runtime（${lib}）。请把自构建产物放到 src-tauri/runtime/${triple}/，或在 src-tauri/runtime/runtime.json 登记下载地址与 SHA-256。`;
  // 显式逃生口：CI 在运行时尚未就绪的平台上仍可产出安装包，但应用内 OCR 引擎会明确显示不可用。
  if (process.env.DREAMPAPER_ALLOW_MISSING_RUNTIME === '1') {
    console.warn(`警告：${message}\n警告：本次安装包将不含 OCR 运行时（DREAMPAPER_ALLOW_MISSING_RUNTIME=1）。`);
    writeConfigOverride(false);
    process.exit(0);
  }
  throw new Error(message);
}
writeConfigOverride(true);

// 3. 校验并暂存到平台配置引用的固定路径。
const bytes = readFileSync(source);
const sha256 = createHash('sha256').update(bytes).digest('hex');
if (pinned?.sha256 && pinned.sha256 !== sha256) {
  throw new Error(`ONNX Runtime 摘要与 runtime.json 不符：${sha256}`);
}
if (!pinned?.sha256) {
  console.warn(`警告：runtime.json 未登记 ${triple} 的 SHA-256，本次使用 ${sha256}`);
}
const ortDir = resolve(root, 'src-tauri/runtime/ort');
mkdirSync(ortDir, { recursive: true });
copyFileSync(source, resolve(ortDir, lib));
writeFileSync(
  resolve(ortDir, 'manifest.json'),
  JSON.stringify(
    {
      triple,
      sidecar: `dreampaper-ocr-${triple}${exe}`,
      sidecar_bytes: statSync(sidecar).size,
      sidecar_sha256: createHash('sha256').update(readFileSync(sidecar)).digest('hex'),
      runtime: lib,
      runtime_bytes: bytes.length,
      runtime_sha256: sha256,
      onnxruntime: manifest.version ?? '1.29.0'
    },
    null,
    2
  ) + '\n'
);
console.log(`SIDECAR_OK ${sidecar}`);
console.log(`RUNTIME_OK ${resolve(ortDir, lib)} sha256=${sha256} bytes=${bytes.length}`);

function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, { stdio: 'inherit' });
  if (result.status !== 0) throw new Error(`${command} ${commandArgs.join(' ')} 退出码 ${result.status}`);
}

// 当前平台的配置直接引用 runtime/ort/<lib>；运行时缺席时把引用置空，Tauri 才不会因文件不存在而中止打包。
// 只改写本平台的文件，正常（present）情况下写回的内容与仓库中一致。
function writeConfigOverride(present) {
  const schema = '../node_modules/@tauri-apps/cli/config.schema.json';
  if (darwin) {
    writeFileSync(
      resolve(root, 'src-tauri/tauri.macos.conf.json'),
      JSON.stringify({ $schema: schema, bundle: { macOS: { frameworks: present ? ['runtime/ort/libonnxruntime.dylib'] : [] } } }, null, 2) + '\n'
    );
  } else if (windows) {
    writeFileSync(
      resolve(root, 'src-tauri/tauri.windows.conf.json'),
      JSON.stringify({ $schema: schema, bundle: { resources: present ? { 'runtime/ort/onnxruntime.dll': 'ort/onnxruntime.dll' } : {} } }, null, 2) + '\n'
    );
  }
}
