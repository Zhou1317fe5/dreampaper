import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const locked = JSON.parse(readFileSync(join(root, 'src-tauri/runtime/runtime.json'), 'utf8'));
export const targets = {
  'x86_64-pc-windows-msvc': { platform: 'win32', arch: 'x64', library: 'onnxruntime.dll', suffix: '.exe' },
  'x86_64-apple-darwin': { platform: 'darwin', arch: 'x64', library: 'libonnxruntime.dylib', suffix: '' },
  'aarch64-apple-darwin': { platform: 'darwin', arch: 'arm64', library: 'libonnxruntime.dylib', suffix: '' }
};

export function targetInfo(triple) {
  const info = targets[triple];
  if (!info) throw new Error(`不支持的目标：${triple}`);
  return info;
}

export function digest(path) {
  const bytes = readFileSync(path);
  return { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
}

export function filesIn(directory, prefix = '') {
  return readdirSync(join(directory, prefix), { withFileTypes: true }).flatMap((entry) => {
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) throw new Error(`运行时目录不允许符号链接：${name}`);
    return entry.isDirectory() ? filesIn(directory, name) : [name];
  }).sort();
}

export function verifyRuntime(directory, triple) {
  const info = targetInfo(triple);
  const manifest = JSON.parse(readFileSync(join(directory, 'manifest.json'), 'utf8'));
  if (manifest.triple !== triple || manifest.commit !== locked.commit || manifest.version !== locked.version) {
    throw new Error('运行时来源或目标与锁定清单不符');
  }
  if (manifest.recipe_sha256 !== digest(join(root, 'scripts/ort.mjs')).sha256) {
    throw new Error('运行时构建配方已变化，请重新构建');
  }
  if (!Array.isArray(manifest.files) || !manifest.files.some((file) => file.name === info.library)) {
    throw new Error(`运行时清单缺少 ${info.library}`);
  }
  const names = new Set();
  for (const file of manifest.files) {
    if (typeof file.name !== 'string' || !/^[\w.-]+(?:\/[\w.-]+)*$/.test(file.name) || file.name.split('/').includes('..') || names.has(file.name)) {
      throw new Error('运行时清单包含无效或重复路径');
    }
    names.add(file.name);
    if (!Number.isSafeInteger(file.bytes) || file.bytes <= 0 || !/^[a-f0-9]{64}$/.test(file.sha256)) {
      throw new Error(`运行时摘要无效：${file.name}`);
    }
    const path = join(directory, file.name);
    if (!statSync(path).isFile()) throw new Error(`运行时文件不存在：${file.name}`);
    const actual = digest(path);
    if (actual.bytes !== file.bytes || actual.sha256 !== file.sha256) throw new Error(`运行时校验失败：${file.name}`);
  }
  const extra = filesIn(directory).filter((name) => name !== 'manifest.json' && !names.has(name));
  if (extra.length) throw new Error(`运行时存在未登记文件：${extra.join(', ')}`);
  return manifest;
}
