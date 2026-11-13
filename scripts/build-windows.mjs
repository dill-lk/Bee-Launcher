import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const packageJson = JSON.parse(await readFile(path.join(repo, 'package.json'), 'utf8'));
const version = packageJson.version;
const target = process.env.BEE_WINDOWS_TARGET || 'x86_64-pc-windows-msvc';
const targetDir = path.resolve(process.env.CARGO_TARGET_DIR || path.join(repo, 'src-tauri', 'target'));
const releaseDir = path.resolve(process.env.BEE_RELEASE_DIR || path.join(repo, 'release-artifacts'));
const runnerArgs = process.platform === 'win32' ? [] : ['--runner', 'cargo-xwin'];
const buildDir = path.join(targetDir, target, 'release');
const launcherExe = path.join(buildDir, 'blockline-launcher.exe');
const nsisDir = path.join(buildDir, 'bundle', 'nsis');
const checksumName = `SHA256SUMS-${version}.txt`;
const setupName = `Bee-Launcher-Setup-${version}-Windows-x64.exe`;
const launcherName = `Bee-Launcher-${version}-Windows-x64.exe`;

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

async function ensureNoOutputCollision(names) {
  await mkdir(releaseDir, { recursive: true });
  if (process.env.BEE_ALLOW_RELEASE_OVERWRITE === '1') return;
  for (const name of [...names, checksumName]) {
    try {
      await readFile(path.join(releaseDir, name));
      throw new Error(`Refusing to overwrite an existing release: ${path.join(releaseDir, name)}`);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
}

function assertWindowsPe(bytes, label) {
  if (bytes.length < 2 || bytes.subarray(0, 2).toString() !== 'MZ') {
    throw new Error(`${label} is not a Windows PE executable.`);
  }
}

await ensureNoOutputCollision([launcherName, setupName]);
console.log(`Building Bee Launcher ${version} for ${target} with Tauri’s NSIS bundle…`);
await run('pnpm', ['exec', 'tauri', 'build', ...runnerArgs, '--target', target, '--ci'], repo);

const launcherBytes = await readFile(launcherExe);
assertWindowsPe(launcherBytes, `Launcher output ${launcherExe}`);

let nsisEntries;
try {
  nsisEntries = await readdir(nsisDir, { withFileTypes: true });
} catch (error) {
  throw new Error(`Tauri did not create the expected NSIS output directory ${nsisDir}: ${error.message}`);
}
const setupCandidates = nsisEntries.filter((entry) =>
  entry.isFile() && entry.name === `Bee Launcher_${version}_x64-setup.exe`,
);
if (setupCandidates.length !== 1) {
  throw new Error(
    `Expected the Tauri NSIS setup executable for ${version} in ${nsisDir}; found ${setupCandidates.length}: ${setupCandidates.map((entry) => entry.name).join(', ') || '(none)'}`,
  );
}
const setupExe = path.join(nsisDir, setupCandidates[0].name);
const setupBytes = await readFile(setupExe);
assertWindowsPe(setupBytes, `NSIS output ${setupExe}`);

const releaseOutputs = [
  [launcherExe, launcherName],
  [setupExe, setupName],
];
const sums = [];
for (const [source, outputName] of releaseOutputs) {
  const bytes = await readFile(source);
  const destination = path.join(releaseDir, outputName);
  await copyFile(source, destination, constants.COPYFILE_EXCL);
  sums.push(`${createHash('sha256').update(bytes).digest('hex')}  ${outputName}`);
  console.log(`Created ${destination} (${bytes.byteLength.toLocaleString()} bytes)`);
}

const checksumPath = path.join(releaseDir, checksumName);
await writeFile(checksumPath, `${sums.join('\n')}\n`, { flag: 'wx' });
console.log(`Checksums written to ${checksumPath}`);
