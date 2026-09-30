const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { getPlatformAsset } = require('./platform');

const packageRoot = path.resolve(__dirname, '..');
if (process.platform !== 'win32') fs.chmodSync(path.join(packageRoot, 'bin', 'agenttrace.js'), 0o755);
const build = spawnSync('cargo', ['build', '--release', '--locked'], {
  cwd: packageRoot,
  stdio: 'inherit',
  shell: process.platform === 'win32',
});

if (build.error) throw build.error;
if (build.status !== 0) process.exit(build.status || 1);

const asset = getPlatformAsset(process.platform, process.arch);
const binaryPath = path.join(packageRoot, 'target', 'release', process.platform === 'win32' ? 'agenttrace.exe' : 'agenttrace');
const vendorDirectory = path.join(packageRoot, 'vendor', asset.directory);
const vendorBinary = path.join(vendorDirectory, asset.filename);
fs.mkdirSync(vendorDirectory, { recursive: true });
fs.copyFileSync(binaryPath, vendorBinary);
if (!asset.filename.endsWith('.exe')) fs.chmodSync(vendorBinary, 0o755);

const verification = spawnSync(process.execPath, [path.join(__dirname, 'verify-package.js')], {
  cwd: packageRoot,
  stdio: 'inherit',
});
if (verification.error) throw verification.error;
if (verification.status !== 0) process.exit(verification.status || 1);

const outputDirectory = path.join(packageRoot, 'target', 'npm');
fs.mkdirSync(outputDirectory, { recursive: true });
const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const packed = spawnSync(npmCommand, ['pack', '--pack-destination', path.join('target', 'npm'), '--cache', '.npm-cache'], {
  cwd: packageRoot,
  stdio: 'inherit',
  shell: process.platform === 'win32',
});
if (packed.error) throw packed.error;
process.exit(packed.status || 0);
