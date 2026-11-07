#!/usr/bin/env node
// Launcher for the Ostler binary. Finds (or downloads, once) the prebuilt binary for this
// platform and runs it with the same arguments, stdio and exit code.
//
// Rules this file keeps, because MCP clients run it as `npx -y ostler serve`:
//  * stdout belongs to the MCP protocol. Everything this launcher says goes to stderr.
//  * No install scripts. The download happens on first run and is verified against a sha256.
//  * OSTLER_BINARY=/path/to/ostler skips all of this and runs that binary.
'use strict';

const { spawnSync } = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');

const log = (m) => process.stderr.write(`ostler: ${m}\n`);
const fail = (m) => { log(m); process.exit(1); };

const version = require('../package.json').version;
const TARGETS = {
  'darwin/arm64': 'aarch64-apple-darwin',
  'darwin/x64': 'x86_64-apple-darwin',
  'linux/x64': 'x86_64-unknown-linux-gnu',
  'linux/arm64': 'aarch64-unknown-linux-gnu',
  'win32/x64': 'x86_64-pc-windows-msvc',
};

function binaryPath() {
  if (process.env.OSTLER_BINARY) return process.env.OSTLER_BINARY;
  const target = TARGETS[`${process.platform}/${process.arch}`];
  if (!target) {
    fail(`no prebuilt binary for ${process.platform}/${process.arch}. Install from source instead: cargo install ostler`);
  }
  const cacheRoot = process.env.OSTLER_CACHE_DIR
    || path.join(process.env.XDG_CACHE_HOME || path.join(os.homedir(), '.cache'), 'ostler');
  const name = `ostler-v${version}-${target}`;
  const bin = path.join(cacheRoot, name, process.platform === 'win32' ? 'ostler.exe' : 'ostler');
  if (fs.existsSync(bin)) return bin;
  return download({ target, name, cacheRoot, bin });
}

async function fetchBuffer(url) {
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok) throw new Error(`${url} returned HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

async function download({ name, cacheRoot, bin }) {
  const base = process.env.OSTLER_RELEASE_BASE || 'https://github.com/inamdarmihir/ostler/releases';
  const url = `${base}/download/v${version}/${name}.tar.gz`;
  log(`first run: downloading ${name} (one time, cached in ${cacheRoot})`);
  let archive, sum;
  try {
    [archive, sum] = await Promise.all([fetchBuffer(url), fetchBuffer(`${url}.sha256`)]);
  } catch (e) {
    fail(`download failed: ${e.message}\n  Is v${version} released for this platform? Alternatives: cargo install ostler, or set OSTLER_BINARY.`);
  }
  const want = sum.toString('utf8').trim().split(/\s+/)[0];
  const got = crypto.createHash('sha256').update(archive).digest('hex');
  if (want !== got) fail(`checksum mismatch (expected ${want}, got ${got}). Not running it.`);

  fs.mkdirSync(cacheRoot, { recursive: true });
  const tmp = fs.mkdtempSync(path.join(cacheRoot, '.tmp-'));
  try {
    const tgz = path.join(tmp, 'a.tar.gz');
    fs.writeFileSync(tgz, archive);
    const r = spawnSync('tar', ['-xzf', tgz, '-C', tmp], { stdio: ['ignore', 'ignore', 'inherit'] });
    if (r.status !== 0) fail('could not unpack the download (is `tar` installed?)');
    const dest = path.join(cacheRoot, name);
    fs.rmSync(dest, { recursive: true, force: true });
    fs.renameSync(path.join(tmp, name), dest); // atomic: a half-unpacked copy is never used
    fs.chmodSync(bin, 0o755);
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
  return bin;
}

(async () => {
  const bin = await Promise.resolve(binaryPath());
  const r = spawnSync(bin, process.argv.slice(2), { stdio: 'inherit' });
  if (r.error) fail(`could not run ${bin}: ${r.error.message}`);
  if (r.signal) process.kill(process.pid, r.signal);
  process.exit(r.status ?? 1);
})();
