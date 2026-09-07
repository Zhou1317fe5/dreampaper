import { createWriteStream } from 'node:fs';
import { access, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { spawn } from 'node:child_process';

const VERSION = '1.29.0';
const COMMIT = '2e2543fbe9fae542f921d47a72d21d5a4ef0b710';
const PATCHES = [
  {
    file: 'onnxruntime/core/mlas/lib/qlutgemm.cpp',
    original: 'auto lut_buffer = std::make_unique_for_overwrite<int8_t[]>(lut_buffer_size);',
    compatible: 'auto lut_buffer = std::make_unique<int8_t[]>(lut_buffer_size);'
  },
  {
    file: 'onnxruntime/core/session/model_editor_c_api.cc',
    original: 'auto [ptr_it, ptr_inserted] = graph->initializer_ptrs.insert(tensor);',
    compatible: 'auto ptr_result = graph->initializer_ptrs.insert(tensor);\n  auto ptr_it = ptr_result.first;\n  bool ptr_inserted = ptr_result.second;'
  },
  {
    file: 'onnxruntime/core/session/model_editor_c_api.cc',
    original: 'auto [ptr_it, ptr_inserted] = graph->node_ptrs.insert(node);',
    compatible: 'auto ptr_result = graph->node_ptrs.insert(node);\n  auto ptr_it = ptr_result.first;\n  bool ptr_inserted = ptr_result.second;'
  }
];
const arch = process.argv[2] ?? (process.arch === 'x64' ? 'x86_64' : process.arch);
if (process.platform !== 'darwin') throw new Error('此脚本仅构建 macOS ONNX Runtime');
if (!['x86_64', 'arm64'].includes(arch)) throw new Error(`不支持的架构：${arch}`);

const root = process.cwd();
const source = resolve(root, `src-tauri/target/gate/onnxruntime-${VERSION}`);
const tools = resolve(root, 'src-tauri/target/gate/tools/bin');
const build = resolve(root, `src-tauri/target/gate/ort-build-${arch}`);
const log = resolve(root, `src-tauri/target/gate/ort-build-${arch}.log`);
for (const path of [source, `${tools}/python`, `${tools}/cmake`, `${tools}/ctest`, `${tools}/ninja`]) {
  await access(path);
}
const head = (await capture('git', ['-C', source, 'rev-parse', 'HEAD'])).trim();
if (head !== COMMIT) throw new Error(`ORT 源码提交不匹配：${head}`);

for (const patch of PATCHES) {
  const file = resolve(source, patch.file);
  const current = await readFile(file, 'utf8');
  const originalCount = count(current, patch.original);
  const compatibleCount = count(current, patch.compatible);
  if (originalCount === 1 && compatibleCount === 0) {
    await writeFile(file, current.replace(patch.original, patch.compatible));
  } else if (originalCount !== 0 || compatibleCount !== 1) {
    throw new Error(`ORT 兼容补丁上下文不匹配：${patch.file}`);
  }
}

const args = [
  `${source}/tools/ci_build/build.py`,
  '--build_dir', build,
  '--config', 'Release',
  '--update', '--build',
  '--build_shared_lib',
  '--skip_tests',
  '--skip_pip_install',
  '--skip_submodule_sync',
  '--compile_no_warning_as_error',
  '--no_telemetry',
  '--parallel', '6',
  '--cmake_generator', 'Ninja',
  '--cmake_path', `${tools}/cmake`,
  '--ctest_path', `${tools}/ctest`,
  '--osx_arch', arch,
  '--apple_deploy_target', '13.0',
  '--cmake_extra_defines',
  'CMAKE_OSX_DEPLOYMENT_TARGET=13.0',
  'CMAKE_BUILD_TYPE=Release'
];

const output = createWriteStream(log, { flags: 'w' });
const code = await run(`${tools}/python`, args, {
  ...process.env,
  PATH: `${tools}:${process.env.PATH ?? ''}`,
  MACOSX_DEPLOYMENT_TARGET: '13.0'
}, output);
output.end();
if (code !== 0) throw new Error(`ORT 构建失败，日志：${log}`);

const dylib = `${build}/Release/libonnxruntime.dylib`;
await access(dylib);
console.log(`ORT_BUILD_OK ${dylib}`);
console.log(await capture('shasum', ['-a', '256', dylib]));
console.log(await capture('otool', ['-L', dylib]));

function count(text, value) {
  return text.split(value).length - 1;
}

function run(command, args, env, stream) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(command, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
    child.stdout.pipe(stream, { end: false });
    child.stderr.pipe(stream, { end: false });
    child.on('error', reject);
    child.on('exit', (code) => resolveRun(code ?? 1));
  });
}

function capture(command, args) {
  return new Promise((resolveRun, reject) => {
    const child = spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    child.stdout.setEncoding('utf8').on('data', (chunk) => { stdout += chunk; });
    child.stderr.setEncoding('utf8').on('data', (chunk) => { stderr += chunk; });
    child.on('error', reject);
    child.on('exit', (code) => code === 0 ? resolveRun(stdout) : reject(new Error(stderr || `${command} 退出：${code}`)));
  });
}
