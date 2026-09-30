const test = require('node:test');
const assert = require('node:assert/strict');
const { getPlatformAsset } = require('../scripts/platform');

test('maps all release targets to their native binary filenames', () => {
  const targets = [
    ['win32', 'x64', 'win32-x64', 'agenttrace.exe'],
    ['win32', 'arm64', 'win32-arm64', 'agenttrace.exe'],
    ['darwin', 'x64', 'darwin-x64', 'agenttrace'],
    ['darwin', 'arm64', 'darwin-arm64', 'agenttrace'],
    ['linux', 'x64', 'linux-x64', 'agenttrace'],
    ['linux', 'arm64', 'linux-arm64', 'agenttrace'],
  ];

  for (const [platform, architecture, directory, filename] of targets) {
    assert.deepEqual(getPlatformAsset(platform, architecture), { directory, filename });
  }
});

test('rejects operating systems and architectures without release binaries', () => {
  assert.throws(() => getPlatformAsset('freebsd', 'x64'), /does not provide a binary/);
  assert.throws(() => getPlatformAsset('linux', 'arm'), /does not provide a binary/);
});
