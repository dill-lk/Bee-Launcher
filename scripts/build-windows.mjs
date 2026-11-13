import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { copyFile, mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const packageJson = JSON.parse(await readFile(path.join(repo, 'package.json'), 'utf8'));
const version = packageJson.version;
const target = process.env.BEE_WINDOWS_TARGET || 'x86_64-pc-windows-msvc';
const targetDir = path.resolve(process.env.CARGO_TARGET_DIR || path.join(repo, 'src-tauri', 'target'));
const releaseDir = path.resolve(process.env.BEE_RELEASE_DIR || path.join(repo, 'release-artifacts'));
const runnerArgs = process.platform === 'win32' ? [] : ['--runner', 'cargo-xwin'];
const launcherExe = path.join(targetDir, target, 'release', 'blockline-launcher.exe');
const setupExe = path.join(targetDir, target, 'release', 'bee-launcher-setup.exe');
const checksumName = `SHA256SUMS-${version}.txt`;
const outputs = [
  [launcherExe, `Bee-Launcher-${version}-Windows-x64.exe`],
  [setupExe, `Bee-Launcher-Setup-${version}-Windows-x64.exe`],
];

function run(command, args, cwd, env = process.env) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env, stdio: 'inherit', windowsHide: true });
    child.on('error', reject);
    child.on('exit', (code, signal) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} exited with ${code ?? signal}`));
    });
  });
}

async function ensureNoOutputCollision() {
  await mkdir(releaseDir, { recursive: true });
  if (process.env.BEE_ALLOW_RELEASE_OVERWRITE === '1') return;
  for (const name of [...outputs.map(([, outputName]) => outputName), checksumName]) {
    try {
      await readFile(path.join(releaseDir, name));
      throw new Error(`Refusing to overwrite an existing release: ${path.join(releaseDir, name)}`);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
}

await ensureNoOutputCollision();
console.log(`Building Bee Launcher ${version} for ${target}…`);
await run('pnpm', ['exec', 'tauri', 'build', ...runnerArgs, '--target', target, '--no-bundle', '--ci'], repo);
const launcherBytes = await readFile(launcherExe);
if (launcherBytes.length < 2 || launcherBytes.subarray(0, 2).toString() !== 'MZ') {
  throw new Error(`Launcher output is not a Windows PE executable: ${launcherExe}`);
}

await run('pnpm', ['run', 'installer:build'], repo);
const setupEnv = {
  ...process.env,
  CARGO_TARGET_DIR: targetDir,
  BEE_LAUNCHER_PAYLOAD: launcherExe,
};
await run(
  'pnpm',
  ['exec', 'tauri', 'build', ...runnerArgs, '--target', target, '--no-bundle', '--ci'],
  path.join(repo, 'installer', 'src-tauri'),
  setupEnv,
);
const setupBytes = await readFile(setupExe);
if (setupBytes.length < 2 || setupBytes.subarray(0, 2).toString() !== 'MZ') {
  throw new Error(`Setup output is not a Windows PE executable: ${setupExe}`);
}

const sums = [];
for (const [source, name] of outputs) {
  const destination = path.join(releaseDir, name);
  const bytes = await readFile(source);
  await copyFile(source, destination);
  sums.push(`${createHash('sha256').update(bytes).digest('hex')}  ${name}`);
  console.log(`Created ${destination} (${bytes.byteLength.toLocaleString()} bytes)`);
}
await writeFile(path.join(releaseDir, checksumName), `${sums.join('\n')}\n`, { flag: 'wx' });
console.log(`Checksums written to ${path.join(releaseDir, checksumName)}`);
