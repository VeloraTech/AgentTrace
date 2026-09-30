const fs = require('node:fs');
const path = require('node:path');

const [target, artifactName] = process.argv.slice(2);
if (!target || !artifactName) throw new Error('Usage: node scripts/copy-binary.js <rust-target> <artifact-name>');

const executable = target.includes('windows') ? 'agenttrace.exe' : 'agenttrace';
const sourcePath = path.join('target', target, 'release', executable);
const artifactPath = path.join('dist', artifactName);

if (!fs.existsSync(sourcePath) || fs.statSync(sourcePath).size === 0) {
  throw new Error(`Cargo did not produce the expected executable: ${sourcePath}`);
}

fs.mkdirSync(path.dirname(artifactPath), { recursive: true });
fs.copyFileSync(sourcePath, artifactPath);
if (!artifactPath.endsWith('.exe')) fs.chmodSync(artifactPath, 0o755);
process.stdout.write(`Prepared ${artifactPath}.\n`);
