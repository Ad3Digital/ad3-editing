/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { createMacOsSigningOptions } from './macos-signing-options.mjs';

test('uses ad-hoc signing and preserves osx-sign defaults when no identity is configured', () => {
  const options = createMacOsSigningOptions(undefined);

  assert.equal(options.identity, '-');
  assert.equal(options.identityValidation, false);
  assert.equal(options.continueOnError, false);
  assert.equal(options.hardenedRuntime, undefined);
  assert.equal(typeof options.optionsForFile, 'function');
  assert.match(options.optionsForFile('/tmp/AD3 Editing.app').entitlements, /macos-default\.plist$/);
  assert.match(
    options.optionsForFile('/tmp/AD3 Editing Helper (GPU).app').entitlements,
    /macos-gpu\.plist$/,
  );
});

test('treats an explicit dash identity as ad-hoc signing', () => {
  const options = createMacOsSigningOptions('-');

  assert.equal(options.identity, '-');
  assert.equal(options.identityValidation, false);
  assert.equal(typeof options.optionsForFile, 'function');
});

test('keeps normal identity validation and osx-sign defaults for Developer ID signing', () => {
  const identity = 'Developer ID Application: AD3 (ABCDE12345)';
  const options = createMacOsSigningOptions(identity);

  assert.equal(options.identity, identity);
  assert.equal(options.identityValidation, true);
  assert.equal(options.optionsForFile, undefined);
});

// Pin the actual entitlement contents, not just their filenames. An osx-sign
// dependency update must not silently drop a newly required runtime entitlement.
test('ad-hoc plists preserve each upstream default and add only library validation', () => {
  const require = createRequire(import.meta.url);
  const requireForge = createRequire(require.resolve('@electron-forge/core'));
  const requirePackager = createRequire(requireForge.resolve('@electron/packager'));
  const signer = requirePackager.resolve('@electron/osx-sign');
  const signerRoot = dirname(requirePackager.resolve('@electron/osx-sign/package.json'));
  const { parse } = createRequire(signer)('plist');
  const options = createMacOsSigningOptions('-');
  for (const [bundle, defaults] of [
    ['AD3 Editing.app', 'default.darwin.plist'],
    ['AD3 Editing Helper.app', 'default.darwin.plist'],
    ['AD3 Editing Helper (GPU).app', 'default.darwin.gpu.plist'],
    ['AD3 Editing Helper (Renderer).app', 'default.darwin.renderer.plist'],
    ['AD3 Editing Helper (Plugin).app', 'default.darwin.plugin.plist'],
  ]) {
    const original = parse(readFileSync(join(signerRoot, 'entitlements', defaults), 'utf8'));
    const actual = parse(readFileSync(options.optionsForFile('/tmp/' + bundle).entitlements, 'utf8'));
    assert.deepEqual(actual, { ...original, 'com.apple.security.cs.disable-library-validation': true }, bundle);
  }
});
