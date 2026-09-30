const { spawnSync } = require('node:child_process');
const path = require('node:path');
const { getPlatformAsset, supportedPlatforms } = require('./platform');

const releasePackage = process.argv.includes('--all-platforms');
const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const packageRoot = path.resolve(__dirname, '..');
const result = spawnSync(npmCommand, ['pack', '--dry-run', '--json', '--cache', '.npm-cache'], {
  cwd: packageRoot,
  encoding: 'utf8',
  shell: process.platform === 'win32',
});

if (result.error) throw result.error;
if (result.status !== 0) throw new Error(result.stderr || `npm pack exited with ${result.status}.`);

const packDetails = JSON.parse(result.stdout)[0];
const files = new Set(packDetails.files.map((file) => file.path));
const requiredFiles = ['package.json', 'README.md', 'LICENSE', 'bin/agenttrace.js', 'scripts/platform.js'];
const platforms = releasePackage
  ? [...supportedPlatforms.keys()]
  : [`${process.platform}-${process.arch}`];

for (const platform of platforms) {
  const [os, architecture] = platform.split('-');
  const asset = getPlatformAsset(os, architecture);
  requiredFiles.push(path.posix.join('vendor', asset.directory, asset.filename));
}

for (const requiredFile of requiredFiles) {
  if (!files.has(requiredFile)) throw new Error(`npm pack is missing required file: ${requiredFile}`);
}

for (const platform of platforms) {
  const [os, architecture] = platform.split('-');
  const asset = getPlatformAsset(os, architecture);
  if (asset.filename.endsWith('.exe')) continue;
  const packedFile = packDetails.files.find((file) => file.path === path.posix.join('vendor', asset.directory, asset.filename));
  if (packedFile.mode !== undefined && (packedFile.mode & 0o111) === 0) {
    throw new Error(`npm pack did not preserve executable permissions for ${packedFile.path}.`);
  }
}

const allowed = new Set(requiredFiles);
const unexpectedFiles = [...files].filter((file) => !allowed.has(file));
if (unexpectedFiles.length) throw new Error(`npm pack contains unexpected files: ${unexpectedFiles.join(', ')}`);

const commandFile = packDetails.files.find((file) => file.path === 'bin/agenttrace.js');
if (process.platform !== 'win32' && commandFile.mode !== undefined && (commandFile.mode & 0o111) === 0) {
  throw new Error('npm pack did not mark the agenttrace command wrapper executable.');
}

process.stdout.write(`npm pack contents verified (${files.size} files; ${platforms.length} platform binary/binaries).\n`);
