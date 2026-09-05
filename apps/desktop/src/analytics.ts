/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { app } from "electron";
import { join } from "node:path";
import { access, writeFile } from "node:fs/promises";

/**
 * Install tracking, proxied by the first launch of a packaged build. The
 * renderer's Umami script never fires inside Electron (its `data-domains`
 * gate does not match the `file://` origin), so main posts one event
 * directly to Umami's send API and drops a marker file in `userData` to
 * never send it again. Offline first launches retry on the next launch.
 */

const UMAMI_ENDPOINT = "https://cloud.umami.is/api/send";
const UMAMI_WEBSITE_ID = "e898e381-6ce3-4f5f-a485-8327ec9aa88b";
const HOSTNAME = "desktop.diffusion.studio";
const MARKER_FILE = "install-tracked";

// Umami discards events whose User-Agent trips its bot filter, which both
// Node's fetch default and Electron's own UA do — send a plain browser UA
// that still attributes the right OS.
function userAgent(): string {
  const platform =
    process.platform === "darwin"
      ? "Macintosh; Intel Mac OS X 10_15_7"
      : process.platform === "win32"
        ? "Windows NT 10.0; Win64; x64"
        : "X11; Linux x86_64";
  return `Mozilla/5.0 (${platform}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36`;
}

export async function trackInstall(): Promise<void> {
  if (!app.isPackaged) return;

  const marker = join(app.getPath("userData"), MARKER_FILE);
  try {
    await access(marker);
    return;
  } catch {
    // No marker yet — this is the first (tracked) launch.
  }

  try {
    const response = await fetch(UMAMI_ENDPOINT, {
      method: "POST",
      headers: { "Content-Type": "application/json", "User-Agent": userAgent() },
      body: JSON.stringify({
        type: "event",
        payload: {
          website: UMAMI_WEBSITE_ID,
          hostname: HOSTNAME,
          url: "/desktop/install",
          name: "desktop_install",
          data: {
            platform: process.platform,
            arch: process.arch,
            version: app.getVersion(),
          },
        },
      }),
    });
    if (response.ok) {
      await writeFile(marker, new Date().toISOString());
    }
  } catch {
    // Offline or Umami unreachable — retried on the next launch.
  }
}
