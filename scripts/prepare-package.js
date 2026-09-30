const fs = require('node:fs');
const path = require('node:path');
const { getPlatformAsset, supportedPlatforms } = require('./platform');

const artifactDirectory = path.resolve(process.argv[2] || 'dist/release');
const packageRoot = path.resolve(__dirname, '..');

for (const directory of supportedPlatforms.keys()) {
  const sourceFilename = `agenttrace-${directory}${directory.startsWith('win32-') ? '.exe' : ''}`;
  const sourcePath = path.join(artifactDirectory, sourceFilename);
  const asset = getPlatformAsset(...directory.split('-'));
  const destinationDirectory = path.join(packageRoot, 'vendor', asset.directory);
  const destinationPath = path.join(destinationDirectory, asset.filename);

  if (!fs.existsSync(sourcePath) || !fs.statSync(sourcePath).isFile() || fs.statSync(sourcePath).size === 0) {
    throw new Error(`Required release binary is missing or empty: ${sourcePath}`);
  }

  fs.mkdirSync(destinationDirectory, { recursive: true });
  fs.copyFileSync(sourcePath, destinationPath);
  if (!asset.filename.endsWith('.exe')) {
    fs.chmodSync(destinationPath, 0o755);
  }
}

process.stdout.write(`Staged all ${supportedPlatforms.size} platform binaries under vendor/.\n`);
