/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import type { ForgeConfig } from '@electron-forge/shared-types';
import { MakerDMG } from '@electron-forge/maker-dmg';
import { MakerZIP } from '@electron-forge/maker-zip';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const { version } = JSON.parse(readFileSync(join(__dirname, '..', '..', 'package.json'), 'utf8'));

const config: ForgeConfig = {
  packagerConfig: {
    name: 'AD3 Editing',
    // The old identifier remains the stable macOS/protocol identity for existing installs.
    appBundleId: 'studio.diffusion.editor',
    appCategoryType: 'public.app-category.video',
    appVersion: version,
    appCopyright: 'Copyright (c) 2026 AD3 Editing contributors. Includes MPL-2.0 components.',
    icon: './assets/icon',
    protocols: [{ name: 'AD3 Editing', schemes: ['diffusion'] }],
    prune: false,
    ignore: (path) =>
      path !== '' &&
      path !== '/package.json' &&
      path !== '/assets' &&
      path !== '/assets/icon.ico' &&
      path !== '/dist' &&
      !path.startsWith('/dist/') &&
      path !== '/web' &&
      !path.startsWith('/web/'),
    // Staged by scripts/stage-cli.mjs and stage-docs.mjs; Electron Forge
    // places both directories below the platform's resources directory.
    extraResource: ['./cli', './docs'],
    osxSign: process.platform === 'darwin' && !process.env.SKIP_SIGN ? {} : undefined,
    osxNotarize:
      process.platform === 'darwin' &&
      process.env.APPLE_ID &&
      process.env.APPLE_PASSWORD &&
      process.env.APPLE_TEAM_ID
        ? {
            appleId: process.env.APPLE_ID,
            appleIdPassword: process.env.APPLE_PASSWORD,
            teamId: process.env.APPLE_TEAM_ID,
          }
        : undefined,
  },
  makers: [
    new MakerZIP({}, ['darwin']),
    new MakerDMG({
      name: `AD3-Editing-${process.arch}`,
      icon: './assets/icon.icns',
      // Dark, on-brand window; @2x sibling is picked up automatically for retina.
      background: './assets/dmg-background.png',
      iconSize: 120,
      additionalDMGOptions: {
        'background-color': '#1c1c1c',
        window: { size: { width: 658, height: 498 } },
      },
      contents: (opts) => [
        { x: 188, y: 217, type: 'file', path: opts.appPath },
        { x: 470, y: 217, type: 'link', path: '/Applications' },
      ],
    }),
  ],
};

export default config;
