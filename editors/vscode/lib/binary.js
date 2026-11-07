'use strict';
// Finding or fetching the `ostler` binary. No VS Code APIs here, so it can be tested with plain Node.
// stdout is not used by the extension, but the same rules as the npm launcher apply: the download is
// verified against a sha256 published next to it, and nothing is run if it does not match.

const { spawnSync } = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');

const TARGETS = {
  'darwin/arm64': 'aarch64-apple-darwin',
  'darwin/x64': 'x86_64-apple-darwin',
  'linux/x64': 'x86_64-unknown-linux-gnu',
  'linux/arm64': 'aarch64-unknown-linux-gnu',
};

function targetFor(platform, arch) {
  return TARGETS[`${platform}/${arch}`] || null;
}

/** First field of a `<hash>  <file>` line. */
function parseChecksum(text) {
  const hash = String(text).trim().split(/\s+/)[0] || '';
  return /^[0-9a-f]{64}$/i.test(hash) ? hash.toLowerCase() : null;
}

function sha256(buf) {
  return crypto.createHash('sha256').update(buf).digest('hex');
}

/** An `ostler` on PATH, or null. */
function onPath(env = process.env) {
  for (const dir of (env.PATH || '').split(path.delimiter)) {
    if (!dir) continue;
    const candidate = path.join(dir, 'ostler');
    try {
      fs.accessSync(candidate, fs.constants.X_OK);
      if (fs.statSync(candidate).isFile()) return candidate;
    } catch { /* not here */ }
  }
  return null;
}

async function fetchBuffer(url, fetchImpl = fetch) {
  const res = await fetchImpl(url, { redirect: 'follow' });
  if (!res.ok) throw new Error(`${url} returned HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

/**
 * Download and verify the release for this platform into `storageDir`. Returns the binary path.
 * `base` is the releases URL (override it to test against a local server).
 */
async function download({ version, target, storageDir, base, fetchImpl }) {
  const name = `ostler-v${version}-${target}`;
  const bin = path.join(storageDir, name, 'ostler');
  if (fs.existsSync(bin)) return bin;
  const url = `${base}/download/v${version}/${name}.tar.gz`;
  const [archive, sumText] = await Promise.all([
    fetchBuffer(url, fetchImpl),
    fetchBuffer(`${url}.sha256`, fetchImpl),
  ]);
  const want = parseChecksum(sumText);
  const got = sha256(archive);
  if (!want || want !== got) {
    throw new Error(`checksum mismatch for ${name} (expected ${want}, got ${got}); not running it`);
  }
  fs.mkdirSync(storageDir, { recursive: true });
  const tmp = fs.mkdtempSync(path.join(storageDir, 'dl-'));
  try {
    const tgz = path.join(tmp, 'a.tar.gz');
    fs.writeFileSync(tgz, archive);
    const r = spawnSync('tar', ['-xzf', tgz, '-C', storageDir], { encoding: 'utf8' });
    if (r.status !== 0) throw new Error(`could not unpack the download: ${r.stderr || r.error}`);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
  if (!fs.existsSync(bin)) throw new Error(`the archive did not contain ${name}/ostler`);
  fs.chmodSync(bin, 0o755);
  return bin;
}

/** The `ostler serve` arguments for the user's settings. */
function serveArgs({ headed, cautious, profileDir }) {
  const args = ['serve'];
  if (headed) args.push('--headed');
  if (cautious) args.push('--require-confirm-submit');
  if (profileDir) args.push('--profile-dir', profileDir);
  return args;
}

module.exports = { targetFor, parseChecksum, sha256, onPath, download, serveArgs };
