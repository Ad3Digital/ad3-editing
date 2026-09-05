/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Compiles the macOS corner-radius addon (src/native/corner_radius.m) into
// dist/corner_radius.node. Node-API is ABI-stable, so a plain clang build
// works across Electron versions; node_api symbols resolve from the Electron
// binary at load time via -undefined dynamic_lookup.

import { execFileSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

if (process.platform !== 'darwin') process.exit(0);

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
mkdirSync(join(root, 'dist'), { recursive: true });

execFileSync(
  'clang',
  [
    join(root, 'src', 'native', 'corner_radius.m'),
    '-o', join(root, 'dist', 'corner_radius.node'),
    '-dynamiclib',
    '-undefined', 'dynamic_lookup',
    '-fobjc-arc',
    '-framework', 'AppKit',
    '-framework', 'QuartzCore',
    '-arch', 'arm64',
    '-arch', 'x86_64',
    '-mmacosx-version-min=11.0',
    '-DNAPI_VERSION=8',
    '-Wall',
    '-Werror',
  ],
  { stdio: 'inherit' },
);
