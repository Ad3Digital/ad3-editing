/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createServer as createTcpServer } from "node:net";
import test from "node:test";
import { probeLoopbackHttp } from "../src/readiness.ts";

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  assert.ok(address && typeof address !== "string");
  return address.port;
}

async function close(server) {
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
}

test("readiness probe accepts local headers without using global fetch", async () => {
  const server = createServer((_, response) => response.writeHead(204).end());
  const port = await listen(server);
  const originalFetch = globalThis.fetch;
  let fetchCalls = 0;
  globalThis.fetch = () => {
    fetchCalls += 1;
    throw new Error("setTypeOfService EINVAL");
  };

  try {
    assert.equal(await probeLoopbackHttp(port), true);
    assert.equal(fetchCalls, 0);
  } finally {
    globalThis.fetch = originalFetch;
    await close(server);
  }
});

test("readiness probe treats a refused connection as not ready", async () => {
  const server = createTcpServer();
  const port = await listen(server);
  await close(server);

  assert.equal(await probeLoopbackHttp(port), false);
});

test("readiness probe destroys a request when the server never sends headers", async () => {
  let connection;
  const connectionClosed = Promise.withResolvers();
  const server = createServer((request) => {
    connection = request.socket;
    request.socket.once("close", connectionClosed.resolve);
  });
  const port = await listen(server);
  const startedAt = Date.now();

  try {
    assert.equal(await probeLoopbackHttp(port, 50), false);
    assert.ok(Date.now() - startedAt < 500);
    const destroyed = await Promise.race([
      connectionClosed.promise.then(() => true),
      new Promise((resolve) => setTimeout(resolve, 250, false)),
    ]);
    assert.equal(destroyed, true, "Probe did not destroy its hung connection.");
  } finally {
    connection?.destroy();
    await close(server);
  }
});
