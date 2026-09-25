import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';
// The initial native preview rollout is Windows-only. Other platforms retain their current backend.
if (process.platform !== 'win32') process.exit(0);
const root = fileURLToPath(new URL('../../../', import.meta.url));
const crate = resolve(root, 'native/preview');
execFileSync(process.env.CARGO || 'cargo', ['build', '--release', '--locked', '--manifest-path', resolve(crate, 'Cargo.toml')], { stdio: 'inherit' });
const output = resolve(root, 'apps/desktop/dist');
mkdirSync(output, { recursive: true });
const name = process.platform === 'win32' ? 'ad3-preview.exe' : 'ad3-preview';
copyFileSync(resolve(crate, 'target/release', name), resolve(output, name));
