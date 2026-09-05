/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { defineConfig, loadEnv } from 'vite'
import solid from 'vite-plugin-solid'
import solidSvg from 'vite-plugin-solid-svg'
import tailwindcss from '@tailwindcss/vite'
import typegpu from 'unplugin-typegpu/vite'
import { resolve } from 'path'
import pkg from '../../package.json'

export default defineConfig(({ mode }) => {
  // The desktop app bundles this build; missing client env would silently ship with auth disabled.
  if (mode === 'desktop') {
    const env = loadEnv(mode, __dirname, '')
    for (const key of ['VITE_SUPABASE_URL', 'VITE_SUPABASE_ANON_KEY']) {
      if (!env[key]) {
        throw new Error(`${key} is not set. Copy apps/web/.env.example to apps/web/.env before building the desktop app.`)
      }
    }
  }

  return {
    plugins: [
      solid(),
      tailwindcss(),
      solidSvg({ defaultAsComponent: true }),
      typegpu(),
    ],
    define: {
      APP_VERSION: JSON.stringify(pkg.version),
    },
    server: {
      port: 5173,
      strictPort: true,
      headers: {
        'Cross-Origin-Opener-Policy': 'same-origin',
        'Cross-Origin-Embedder-Policy': 'credentialless',
      },
      proxy: {
        '/api': {
          target: 'http://localhost:3000',
          changeOrigin: true,
        },
      },
    },
    resolve: {
      alias: {
        "@": resolve(__dirname, "./src"),
        "@desktop": resolve(__dirname, "../desktop/src"),
      }
    }
  }
})
