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
const npmDirectory = path.join(artifactDirectory, '..', 'npm');
const npmTarballs = fs.readdirSync(npmDirectory).filter((filename) => filename.endsWith('.tgz'));
if (npmTarballs.length !== 1) throw new Error(`Expected exactly one npm tarball in ${npmDirectory}; found ${npmTarballs.length}.`);
const [npmTarball] = npmTarballs;
const npmTarballPath = path.join(npmDirectory, npmTarball);
if (fs.statSync(npmTarballPath).size === 0) throw new Error(`The npm package tarball is empty: ${npmTarballPath}`);
const npmDigest = crypto.createHash('sha256').update(fs.readFileSync(npmTarballPath)).digest('hex');
checksumLines.push(`${npmDigest}  ${npmTarball}`);

fs.writeFileSync(path.join(artifactDirectory, 'SHA256SUMS'), `${checksumLines.join('\n')}\n`);
process.stdout.write('Wrote SHA256SUMS for the six binaries and npm tarball.\n');
