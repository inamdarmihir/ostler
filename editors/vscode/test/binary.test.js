'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const b = require('../lib/binary');

test('targetFor maps supported platforms only', () => {
  assert.equal(b.targetFor('darwin', 'arm64'), 'aarch64-apple-darwin');
  assert.equal(b.targetFor('linux', 'x64'), 'x86_64-unknown-linux-gnu');
  assert.equal(b.targetFor('win32', 'x64'), null);
});

test('parseChecksum accepts a sha256 line and rejects junk', () => {
  const h = 'a'.repeat(64);
  assert.equal(b.parseChecksum(`${h}  file.tar.gz\n`), h);
  assert.equal(b.parseChecksum('nope'), null);
  assert.equal(b.parseChecksum(''), null);
});

test('serveArgs follows the settings', () => {
  assert.deepEqual(b.serveArgs({}), ['serve']);
  assert.deepEqual(
    b.serveArgs({ headed: true, cautious: true, profileDir: '/p' }),
    ['serve', '--headed', '--require-confirm-submit', '--profile-dir', '/p'],
  );
});

test('onPath finds an executable and ignores a missing one', () => {
  const d = fs.mkdtempSync(path.join(os.tmpdir(), 'ostler-path-'));
  assert.equal(b.onPath({ PATH: d }), null);
  const f = path.join(d, 'ostler');
  fs.writeFileSync(f, '#!/bin/sh\n');
  fs.chmodSync(f, 0o755);
  assert.equal(b.onPath({ PATH: `/nonexistent${path.delimiter}${d}` }), f);
});

function serve(files) {
  return new Promise((resolve) => {
    const s = http.createServer((req, res) => {
      const body = files[req.url];
      if (!body) { res.writeHead(404).end(); return; }
      res.writeHead(200).end(body);
    });
    s.listen(0, '127.0.0.1', () => resolve(s));
  });
}

function fakeRelease(version, target, tamper) {
  const work = fs.mkdtempSync(path.join(os.tmpdir(), 'ostler-rel-'));
  const name = `ostler-v${version}-${target}`;
  fs.mkdirSync(path.join(work, name));
  fs.writeFileSync(path.join(work, name, 'ostler'), '#!/bin/sh\necho fake\n');
  const tgz = path.join(work, `${name}.tar.gz`);
  assert.equal(spawnSync('tar', ['-czf', tgz, '-C', work, name]).status, 0);
  const archive = fs.readFileSync(tgz);
  const sum = tamper ? 'b'.repeat(64) : b.sha256(archive);
  return { [`/download/v${version}/${name}.tar.gz`]: archive, [`/download/v${version}/${name}.tar.gz.sha256`]: `${sum}  ${name}.tar.gz\n` };
}

test('download verifies the checksum, unpacks, and caches', async () => {
  const server = await serve(fakeRelease('9.9.9', 'x86_64-unknown-linux-gnu'));
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ostler-store-'));
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    const bin = await b.download({ version: '9.9.9', target: 'x86_64-unknown-linux-gnu', storageDir: dir, base });
    assert.equal(spawnSync(bin, { encoding: 'utf8' }).stdout.trim(), 'fake');
    server.close();
    // Second call needs no network.
    assert.equal(await b.download({ version: '9.9.9', target: 'x86_64-unknown-linux-gnu', storageDir: dir, base }), bin);
  } finally {
    server.close();
  }
});

test('download refuses a checksum mismatch and leaves nothing runnable', async () => {
  const server = await serve(fakeRelease('9.9.8', 'x86_64-unknown-linux-gnu', true));
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ostler-store-'));
  try {
    await assert.rejects(
      b.download({ version: '9.9.8', target: 'x86_64-unknown-linux-gnu', storageDir: dir, base: `http://127.0.0.1:${server.address().port}` }),
      /checksum mismatch/,
    );
    assert.equal(fs.existsSync(path.join(dir, 'ostler-v9.9.8-x86_64-unknown-linux-gnu')), false);
  } finally {
    server.close();
  }
});

test('download reports a missing release', async () => {
  const server = await serve({});
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ostler-store-'));
  try {
    await assert.rejects(
      b.download({ version: '0.0.1', target: 'x86_64-unknown-linux-gnu', storageDir: dir, base: `http://127.0.0.1:${server.address().port}` }),
      /HTTP 404/,
    );
  } finally {
    server.close();
  }
});
