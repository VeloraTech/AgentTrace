const supportedPlatforms = new Map([
  ['win32-x64', 'agenttrace.exe'],
  ['win32-arm64', 'agenttrace.exe'],
  ['darwin-x64', 'agenttrace'],
  ['darwin-arm64', 'agenttrace'],
  ['linux-x64', 'agenttrace'],
  ['linux-arm64', 'agenttrace'],
]);

function getPlatformAsset(platform, architecture) {
  const directory = `${platform}-${architecture}`;
  const filename = supportedPlatforms.get(directory);

  if (!filename) {
    throw new Error(`AgentTrace does not provide a binary for ${directory}. Supported platforms: ${[...supportedPlatforms.keys()].join(', ')}.`);
  }

  return { directory, filename };
}

module.exports = { getPlatformAsset, supportedPlatforms };
