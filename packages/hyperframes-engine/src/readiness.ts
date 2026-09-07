/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { request } from "node:http";

/**
 * Checks whether the local preview server has accepted an HTTP request without
 * using Electron's bundled fetch/undici implementation.
 */
export function probeLoopbackHttp(port: number, timeoutMs = 1_000): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false;
    const settle = (ready: boolean) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      resolve(ready);
    };
    const probe = request({
      agent: false,
      host: "127.0.0.1",
      method: "GET",
      path: "/",
      port,
    }, (response) => {
      const ready = (response.statusCode ?? 0) > 0;
      response.resume();
      response.destroy();
      settle(ready);
    });
    const timeout = setTimeout(() => {
      probe.destroy();
      settle(false);
    }, timeoutMs);

    probe.once("error", () => settle(false));
    probe.end();
  });
}
