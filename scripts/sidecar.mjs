import { cpSync, copyFileSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { digest, root, targetInfo, verifyRuntime } from './runtime.mjs';

const args = process.argv.slice(2);
const explicit = args.indexOf('--target');
const host = execFileSync('rustc', ['--print', 'host-tuple'], { encoding: 'utf8' }).trim();
const triple = explicit >= 0 ? args[explicit + 1] : host;
const info = targetInfo(triple);
const source = resolve(root, 'src-tauri/runtime', triple);
const manifest = verifyRuntime(source, triple);
const cargoArgs = [
  'build', '--locked', '--release', '--manifest-path', resolve(root, 'src-tauri/ocr/Cargo.toml'),
  '--bin', 'dreampaper-ocr'
];
if (triple !== host) cargoArgs.push('--target', triple);
const env = { ...process.env };
if (info.platform === 'win32') env.RUSTFLAGS = `${env.RUSTFLAGS || ''} -C target-feature=+crt-static`.trim();
const result = spawnSync('cargo', cargoArgs, { stdio: 'inherit', env });
if (result.error) throw result.error;
if (result.status !== 0) throw new Error(`OCR 辅助进程构建失败：${result.status}`);
const built = resolve(root, 'src-tauri/ocr/target', triple !== host ? triple : '', 'release', `dreampaper-ocr${info.suffix}`);
const binaries = resolve(root, 'src-tauri/binaries');
mkdirSync(binaries, { recursive: true });
const sidecar = join(binaries, `dreampaper-ocr-${triple}${info.suffix}`);
copyFileSync(built, sidecar);
const destination = resolve(root, 'src-tauri/runtime/ort');
rmSync(destination, { recursive: true, force: true });
cpSync(source, destination, { recursive: true });
writeFileSync(join(destination, 'sidecar.json'), JSON.stringify({
  triple, runtime: manifest,
  sidecar: { name: `dreampaper-ocr${info.suffix}`, ...digest(sidecar) }
}, null, 2) + '\n');
console.log(`SIDECAR_OK ${sidecar}`);
console.log(`RUNTIME_OK ${join(destination, info.library)}`);
