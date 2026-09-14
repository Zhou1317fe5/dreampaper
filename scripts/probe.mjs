import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { execFileSync, spawnSync, spawn as spawnProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { root, targetInfo, digest } from './runtime.mjs';

const [triple, version] = process.argv.slice(2);
const info = targetInfo(triple);
if (process.platform !== info.platform || process.arch !== info.arch) throw new Error('门禁必须在目标平台运行');
const artifacts = resolve(root, 'release-artifacts');
const reports = resolve(root, 'release-reports', triple);
mkdirSync(reports, { recursive: true });
const sandbox = mkdtempSync(join(tmpdir(), 'dreampaper 测试 '));
const denied = createServer((socket) => { socket.destroy(); });
await new Promise((done, reject) => { denied.once('error', reject); denied.listen(0, '127.0.0.1', done); });
const offline = `http://127.0.0.1:${denied.address().port}`;
let mounted;
const evidence = [];
try {
  const variants = info.platform === 'win32' ? ['setup', 'portable'] : ['dmg'];
  for (const kind of variants) {
    const destination = join(sandbox, kind);
    mkdirSync(destination, { recursive: true });
    let executable;
    let artifact;
    if (kind === 'setup') {
      artifact = join(artifacts, `dreampaper-${version}-setup.exe`);
      const install = join(destination, '安装目录');
      command('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', '$p = Start-Process -FilePath $env:DP_SETUP -ArgumentList @(\'/S\', (\'/D=\' + $env:DP_INSTALL)) -Wait -PassThru; exit $p.ExitCode'], { DP_SETUP: artifact, DP_INSTALL: install });
      executable = join(install, 'dreampaper.exe');
    } else if (kind === 'portable') {
      artifact = join(artifacts, `dreampaper-${version}-portable.zip`);
      command('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', 'Expand-Archive -LiteralPath $env:DP_ARCHIVE -DestinationPath $env:DP_INSTALL'], { DP_ARCHIVE: artifact, DP_INSTALL: destination });
      executable = join(destination, `dreampaper-${version}-portable/dreampaper.exe`);
    } else {
      artifact = join(artifacts, `dreampaper-${version}-${info.arch === 'x64' ? 'x64' : 'arm64'}-mac.dmg`);
      mounted = join(sandbox, 'mount');
      command('hdiutil', ['attach', artifact, '-readonly', '-nobrowse', '-mountpoint', mounted]);
      const apps = readdirSync(mounted).filter((name) => name.endsWith('.app'));
      if (apps.length !== 1) throw new Error('DMG 必须只包含一个 App');
      const app = join(destination, apps[0]);
      command('ditto', [join(mounted, apps[0]), app]);
      command('hdiutil', ['detach', mounted]); mounted = undefined;
      command('codesign', ['--verify', '--deep', '--strict', '--verbose=2', app]);
      executable = join(app, 'Contents/MacOS/dreampaper');
    }
    if (!existsSync(executable)) throw new Error(`最终产物缺少主程序：${executable}`);
    const bin = dirname(executable);
    const resources = info.platform === 'darwin' ? resolve(bin, '../Resources') : bin;
    const runtime = info.platform === 'darwin' ? resolve(bin, '../Frameworks', info.library) : join(bin, 'ort', info.library);
    const sidecar = join(bin, `dreampaper-ocr${info.suffix}`);
    const manifest = JSON.parse(readFileSync(join(resources, 'ort/sidecar.json'), 'utf8'));
    if (manifest.triple !== triple) throw new Error('包内 OCR 架构清单错误');
    const dependencies = inspect([executable, sidecar, runtime]);
    if (info.platform === 'darwin') {
      const permissions = execFileSync('codesign', ['-d', '--entitlements', ':-', sidecar], { encoding: 'utf8' });
      if (!permissions.includes('com.apple.security.cs.disable-library-validation')) throw new Error('OCR 辅助程序缺少 ad-hoc 库加载权限');
      const mainPermissions = execFileSync('codesign', ['-d', '--entitlements', ':-', executable], { encoding: 'utf8' });
      if (mainPermissions.includes('com.apple.security.cs.disable-library-validation')) throw new Error('主程序不应放宽库加载验证');
    }
    if (info.platform === 'win32') {
      if (digest(sidecar).sha256 !== manifest.sidecar.sha256) throw new Error('包内辅助程序摘要不符');
      for (const file of manifest.runtime.files) {
        if (digest(join(resources, 'ort', file.name)).sha256 !== file.sha256) throw new Error(`包内 ORT 文件摘要不符：${file.name}`);
      }
    }
    const work = join(destination, '全新数据'); mkdirSync(work);
    const minimalPath = info.platform === 'win32' ? `${process.env.SystemRoot}\\System32;${process.env.SystemRoot}` : '/usr/bin:/bin';
    const baseEnv = { ...process.env, PATH: minimalPath, PYTHONPATH: '', PYTHONHOME: '', DYLD_LIBRARY_PATH: '', DYLD_FALLBACK_LIBRARY_PATH: '', DREAMPAPER_OCR_SIDECAR: '/nonexistent', DREAMPAPER_OCR_RUNTIME: '/nonexistent' };
    const run = async (mode, disconnected = false) => {
      const report = join(work, `${mode}.json`);
      rmSync(report, { force: true });
      const env = { ...baseEnv, ...(disconnected ? { HTTP_PROXY: offline, HTTPS_PROXY: offline, ALL_PROXY: offline, NO_PROXY: '', http_proxy: offline, https_proxy: offline, all_proxy: offline, no_proxy: '' } : {}) };
      try {
        await spawn(executable, ['--ocr-probe', mode, work], { cwd: work, env });
      } finally {
        const saved = join(reports, kind);
        mkdirSync(saved, { recursive: true });
        for (const name of readdirSync(work, { withFileTypes: true }).filter((entry) => entry.isFile()).map((entry) => entry.name)) {
          copyFileSync(join(work, name), join(saved, name));
        }
      }
      const data = JSON.parse(readFileSync(report, 'utf8'));
      if (!data.ok || data.version !== version) throw new Error(`门禁失败 ${kind}/${mode}: ${JSON.stringify(data)}`);
      return data;
    };
    await run('prepare');
    await run('cancel');
    await run('install');
    await run('offline', true);
    await run('remove');
    await run('install');
    await run('offline', true);
    evidence.push({ kind, artifact: basename(artifact), ...digest(artifact), engine: digest(runtime), sidecar: digest(sidecar), dependencies });
    if (kind === 'setup') {
      const uninstall = readdirSync(bin).find((name) => /^uninstall.*\.exe$/i.test(name));
      if (uninstall) command(join(bin, uninstall), ['/S']);
    }
  }
  writeFileSync(join(reports, 'summary.json'), JSON.stringify({ ok: true, triple, version, os: process.platform, arch: process.arch, minimum_os_verified: false, evidence }, null, 2));
  console.log(`PACKAGE_OCR_OK ${triple}`);
} finally {
  denied.close();
  if (mounted) command('hdiutil', ['detach', mounted]);
  rmSync(sandbox, { recursive: true, force: true });
}

function command(program, args, extraEnv = {}) {
  const result = spawnSync(program, args, { stdio: 'inherit', env: { ...process.env, ...extraEnv }, timeout: 600000 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${program} 退出码 ${result.status}`);
}
function spawn(program, args, options) {
  return new Promise((resolve, reject) => {
    const child = spawnProcess(program, args, { ...options, stdio: 'inherit', timeout: 1500000 });
    child.once('error', reject);
    child.once('exit', (code, signal) => code === 0 ? resolve() : reject(new Error(`门禁进程失败：${code ?? signal}`)));
  });
}
function inspect(files) {
  return files.map((file) => {
    if (info.platform === 'win32') {
      const output = execFileSync('dumpbin', ['/DEPENDENTS', file], { encoding: 'utf8' });
      if (/VCRUNTIME|MSVCP/i.test(output)) throw new Error(`不允许依赖 runner 预装的 MSVC 运行时：${file}`);
      for (const name of output.match(/\b[\w.-]+\.dll\b/gi) || []) {
        if (/^(api-ms-|ext-ms-)/i.test(name)) continue;
        if (!existsSync(join(dirname(file), name)) && !existsSync(join(process.env.SystemRoot, 'System32', name))) throw new Error(`Windows 缺少动态依赖：${name}`);
      }
      return { file: basename(file), output };
    }
    const arch = execFileSync('lipo', ['-archs', file], { encoding: 'utf8' }).trim();
    if (arch !== (info.arch === 'x64' ? 'x86_64' : 'arm64')) throw new Error(`Mach-O 架构错误：${file} ${arch}`);
    const output = execFileSync('otool', ['-L', file], { encoding: 'utf8' });
    for (const dependency of output.split('\n').slice(1).map((line) => line.trim().split(' (')[0]).filter(Boolean)) {
      if (dependency.startsWith('/usr/lib/') || dependency.startsWith('/System/Library/')) continue;
      if (!dependency.startsWith('@')) throw new Error(`包内残留非系统绝对依赖：${dependency}`);
    }
    return { file: basename(file), arch, output };
  });
}
