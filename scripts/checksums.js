const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { supportedPlatforms } = require('./platform');

const artifactDirectory = path.resolve(process.argv[2] || 'dist/release');
const checksumLines = [];

for (const platform of supportedPlatforms.keys()) {
  const filename = `agenttrace-${platform}${platform.startsWith('win32-') ? '.exe' : ''}`;
  const artifactPath = path.join(artifactDirectory, filename);
  const digest = crypto.createHash('sha256').update(fs.readFileSync(artifactPath)).digest('hex');
  checksumLines.push(`${digest}  ${filename}`);
}

const npmPackage = JSON.parse(fs.readFileSync('package.json', 'utf8'));
const npmTarball = `${npmPackage.name}-${npmPackage.version}.tgz`;
const npmTarballPath = path.join(artifactDirectory, '..', 'npm', npmTarball);
if (!fs.existsSync(npmTarballPath) || fs.statSync(npmTarballPath).size === 0) {
  throw new Error(`The npm package tarball is missing or empty: ${npmTarballPath}`);
}
const npmDigest = crypto.createHash('sha256').update(fs.readFileSync(npmTarballPath)).digest('hex');
checksumLines.push(`${npmDigest}  ${npmTarball}`);

fs.writeFileSync(path.join(artifactDirectory, 'SHA256SUMS'), `${checksumLines.join('\n')}\n`);
process.stdout.write('Wrote SHA256SUMS for the six binaries and npm tarball.\n');
