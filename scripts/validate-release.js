const fs = require('node:fs');

const tag = process.argv[2] || process.env.GITHUB_REF_NAME;
const semverPattern = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const match = semverPattern.exec(tag || '');

if (!match) {
  throw new Error(`Expected a stable version tag like v0.1.0; received ${tag || '(missing)'}.`);
}

const version = match[0].slice(1);
const cargoToml = fs.readFileSync('Cargo.toml', 'utf8');
const cargoVersion = /^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m.exec(cargoToml)?.[1];
const npmPackage = JSON.parse(fs.readFileSync('package.json', 'utf8'));

if (cargoVersion !== version || npmPackage.version !== version) {
  throw new Error(`Tag ${tag} must match Cargo (${cargoVersion || 'missing'}) and npm (${npmPackage.version || 'missing'}) versions.`);
}

if (npmPackage.name !== '@coachlogic/agenttrace') {
  throw new Error(`Expected npm package name "@coachlogic/agenttrace"; found "${npmPackage.name}".`);
}

if (process.env.GITHUB_OUTPUT) {
  fs.appendFileSync(process.env.GITHUB_OUTPUT, `version=${version}\n`);
}

process.stdout.write(`Release tag ${tag} matches Cargo and npm version ${version}.\n`);
