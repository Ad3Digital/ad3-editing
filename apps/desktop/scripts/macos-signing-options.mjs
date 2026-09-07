/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const entitlementsDir = join(dirname(fileURLToPath(import.meta.url)), '..', 'entitlements');

const helperEntitlements = [
  ['(Plugin).app', 'macos-plugin.plist'],
  ['(GPU).app', 'macos-gpu.plist'],
  ['(Renderer).app', 'macos-renderer.plist'],
];

export function isDeveloperIdSigning(identity) {
  return Boolean(identity && identity !== '-');
}

/**
 * Keeps @electron/osx-sign's per-file hardened-runtime defaults while adding
 * the library-validation exception required by an ad-hoc Electron bundle.
 */
export function createMacOsSigningOptions(identity) {
  if (!isDeveloperIdSigning(identity)) {
    return {
      identity: '-',
      identityValidation: false,
      continueOnError: false,
      optionsForFile(filePath) {
        const entitlement = helperEntitlements.find(([helper]) => filePath.includes(helper));
        return {
          entitlements: join(entitlementsDir, entitlement?.[1] ?? 'macos-default.plist'),
        };
      },
    };
  }

  return {
    identity,
    identityValidation: true,
    continueOnError: false,
  };
}
