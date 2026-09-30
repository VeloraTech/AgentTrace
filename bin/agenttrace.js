#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { getPlatformAsset } = require('../scripts/platform');

let platformAsset;
try {
  platformAsset = getPlatformAsset(process.platform, process.arch);
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exit(1);
}

const packageRoot = path.join(__dirname, '..');
const executablePath = path.join(packageRoot, 'vendor', platformAsset.directory, platformAsset.filename);
const developmentExecutable = path.join(
  packageRoot,
  'target',
  'release',
  process.platform === 'win32' ? 'agenttrace.exe' : 'agenttrace',
);
const selectedExecutable = fs.existsSync(executablePath) ? executablePath : developmentExecutable;

if (!fs.existsSync(selectedExecutable)) {
  process.stderr.write(`AgentTrace binary for ${platformAsset.directory} is missing. Reinstall the package or build it locally with npm run build:release.\n`);
  process.exit(1);
}

const result = spawnSync(selectedExecutable, process.argv.slice(2), { stdio: 'inherit' });
if (result.error) {
  process.stderr.write(`Could not start AgentTrace: ${result.error.message}\n`);
  process.exit(1);
}

process.exit(result.status ?? 1);
