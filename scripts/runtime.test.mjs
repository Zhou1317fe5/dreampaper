import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { digest, filesIn, locked, root, targetInfo, verifyRuntime } from './runtime.mjs';

function fixture(run) {
  const directory = mkdtempSync(join(tmpdir(), 'runtime 空格 '));
  try {
    mkdirSync(join(directory, 'licenses'));
    writeFileSync(join(directory, 'onnxruntime.dll'), 'test-runtime');
    writeFileSync(join(directory, 'licenses/LICENSE'), 'test-license');
    const manifest = {
      triple: 'x86_64-pc-windows-msvc', commit: locked.commit, version: locked.version,
      recipe_sha256: digest(join(root, 'scripts/ort.mjs')).sha256,
      files: filesIn(directory).map((name) => ({ name, ...digest(join(directory, name)) }))
    };
    const save = () => writeFileSync(join(directory, 'manifest.json'), JSON.stringify(manifest));
    save(); run(directory, manifest, save);
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

test('文件 URL 正确解码 Windows 盘符、空格和中文', () => {
  assert.equal(fileURLToPath('file:///D:/a/%E4%B8%AD%E6%96%87%20repo/scripts/', { windows: true }), 'D:\\a\\中文 repo\\scripts\\');
  assert.equal(targetInfo('aarch64-apple-darwin').arch, 'arm64');
  assert.equal(targetInfo('x86_64-pc-windows-msvc').suffix, '.exe');
  assert.throws(() => targetInfo('unsupported'));
});
test('完整运行时清单可以验证', () => fixture((directory) => {
  assert.equal(verifyRuntime(directory, 'x86_64-pc-windows-msvc').commit, locked.commit);
}));
test('缺库、篡改、额外依赖均阻止打包', () => fixture((directory) => {
  writeFileSync(join(directory, 'extra.dll'), 'extra');
  assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'), /未登记/);
  rmSync(join(directory, 'extra.dll'));
  writeFileSync(join(directory, 'onnxruntime.dll'), 'test-runtimX');
  assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'), /校验失败/);
  rmSync(join(directory, 'onnxruntime.dll'));
  assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'));
}));
test('空摘要、错误源码、错误架构或配方均失败', () => {
  for (const change of [
    (m) => { m.files[0].sha256 = ''; },
    (m) => { m.commit = 'incorrect'; },
    (m) => { m.triple = 'aarch64-apple-darwin'; },
    (m) => { m.recipe_sha256 = '0'.repeat(64); }
  ]) fixture((directory, manifest, save) => {
    change(manifest); save(); assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'));
  });
});
test('拒绝路径穿越和重复清单项', () => {
  for (const name of ['../secret', '/absolute', 'C:/secret', 'licenses/../secret']) fixture((directory, manifest, save) => {
    manifest.files[0].name = name; save(); assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'));
  });
  fixture((directory, manifest, save) => {
    manifest.files.push(manifest.files[0]); save(); assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'), /重复/);
  });
});
test('正式流程禁止缺引擎逃生口，测试必须晚于引擎暂存', () => {
  const workflow = readFileSync(join(root, '.github/workflows/release.yml'), 'utf8');
  assert.ok(!workflow.includes('DREAMPAPER_ALLOW_MISSING_RUNTIME'));
  assert.ok(workflow.indexOf('scripts/sidecar.mjs') < workflow.indexOf('cargo test'));
  assert.ok(workflow.includes('needs: build'));
  assert.ok(workflow.indexOf('scripts/probe.mjs') < workflow.indexOf('name: 上传已验收产物'));
  assert.ok(readFileSync(join(root, 'scripts/draft.mjs'), 'utf8').includes('portable.zip'));
});

test('macOS 动态库加载例外仅限辅助程序，最终包重新签名', () => {
  const bundle = readFileSync(join(root, 'scripts/bundle.mjs'), 'utf8');
  assert.ok(bundle.includes("join(app, 'Contents/MacOS/dreampaper-ocr')"));
  assert.ok(bundle.includes("['--force', '--sign', '-', '--options', 'runtime', app]"));
  assert.ok(bundle.includes("command('hdiutil', ['create'"));
  const config = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.macos.conf.json'), 'utf8'));
  assert.ok(!config.bundle.macOS.entitlements);
  assert.notEqual(config.bundle.macOS.hardenedRuntime, false);
});
