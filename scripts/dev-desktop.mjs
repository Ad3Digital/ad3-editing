/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// One command to develop the desktop app from source. It:
//   1. builds the CLI, so a linked `dapi` (see symlink:create) runs the
//      latest code and the app's headless server matches it;
//   2. starts the web dev server (Vite on :5173), first reclaiming the port
//      from a Vite left behind by an earlier run that did not come down;
//   3. waits for that server, then launches Electron, which loads it.
// Ctrl-C tears the whole tree down.

import { spawn, execFileSync } from "node:child_process";
import { get } from "node:http";
import { setTimeout as delay } from "node:timers/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const BIN = join(ROOT, "node_modules", ".bin");
const DEV_PORT = 5173;
const WINDOWS = process.platform === "win32";
const ROOT_MARKER = ROOT.replace(/[\\/]$/, "").toLowerCase();
const DEV_URL = `http://localhost:${DEV_PORT}`;
const children = [];
let shuttingDown = false;

function runNpm(args) {
  if (WINDOWS) {
    execFileSync(process.env.ComSpec ?? "cmd.exe", ["/d", "/s", "/c", ["npm", ...args].join(" ")], {
      stdio: "inherit",
    });
  } else {
    execFileSync("npm", args, { stdio: "inherit" });
  }
}

function run(name, bin, args, cwd) {
  // Each tool owns a process tree. Windows needs a command shell for .cmd
  // shims; taskkill /T tears that shell and every descendant down together.
  const executable = join(BIN, WINDOWS ? `${bin}.cmd` : bin);
  const child = spawn(executable, args, {
    cwd,
    stdio: "inherit",
    detached: true,
    shell: WINDOWS,
  });
  child.on("exit", (code) => {
    if (shuttingDown) return;
    console.error(`\n[dev:desktop] ${name} exited (${code}); shutting down.`);
    shutdown(code ?? 1);
  });
  children.push(child);
  return child;
}

function shutdown(code) {
  if (shuttingDown) return;
  shuttingDown = true;
  for (const child of children) {
    try {
      if (WINDOWS) {
        execFileSync("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore" });
      } else {
        process.kill(-child.pid, "SIGTERM");
      }
    } catch {
      // Already gone.
    }
  }
  process.exit(code);
}

// Resolves once the dev server answers. Probes over HTTP against the same URL
// Electron loads, so we follow its host resolution (Vite binds localhost as
// IPv6 ::1) rather than guessing an address family.
function waitForServer(url, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  const { promise, resolve, reject } = Promise.withResolvers();
  const tryOnce = () => {
    const req = get(url, (res) => {
      res.destroy();
      resolve();
    });
    req.once("error", () => {
      req.destroy();
      if (Date.now() > deadline) {
        reject(new Error(`Vite did not come up at ${url} in time`));
      } else {
        setTimeout(tryOnce, 200);
      }
    });
  };
  tryOnce();
  return promise;
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => shutdown(0));
}

/** PIDs listening on a TCP port; [] when none or the platform query fails. */
function listeners(port) {
  try {
    const out = WINDOWS
      ? execFileSync(
          "powershell.exe",
          [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            `(Get-NetTCPConnection -State Listen -LocalPort ${port} -ErrorAction SilentlyContinue).OwningProcess | Sort-Object -Unique`,
          ],
          { stdio: ["ignore", "pipe", "ignore"] },
        )
      : execFileSync("lsof", ["-nP", "-t", `-iTCP:${port}`, "-sTCP:LISTEN"], {
          stdio: ["ignore", "pipe", "ignore"],
        });
    return out.toString().split(/\r?\n/).map((line) => Number(line.trim())).filter(Boolean);
  } catch {
    return [];
  }
}

/** The command line of a process, or "" when it is gone. */
function commandOf(pid) {
  try {
    const out = WINDOWS
      ? execFileSync(
          "powershell.exe",
          [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            `(Get-CimInstance Win32_Process -Filter "ProcessId = ${pid}").CommandLine`,
          ],
          { stdio: ["ignore", "pipe", "ignore"] },
        )
      : execFileSync("ps", ["-o", "command=", "-p", String(pid)], {
          stdio: ["ignore", "pipe", "ignore"],
        });
    return out.toString().trim();
  } catch {
    return "";
  }
}

/**
 * Frees the dev port. A Vite left over from an earlier run of this repo
 * (Ctrl-C'd terminal, crashed Electron, a detached child) is killed and the
 * port awaited; anything else on the port is not ours to touch, so we say
 * what it is and stop.
 */
async function reclaimPort(port) {
  const pids = listeners(port);
  if (!pids.length) return;
  for (const pid of pids) {
    const command = commandOf(pid);
    if (!command.toLowerCase().includes("vite") || !command.toLowerCase().includes(ROOT_MARKER)) {
      console.error(`[dev:desktop] port ${port} is in use by another process (pid ${pid}): ${command || "unknown"}`);
      process.exit(1);
    }
    console.log(`[dev:desktop] port ${port} held by a stale vite (pid ${pid}); stopping it…`);
    try {
      if (WINDOWS) {
        execFileSync("taskkill.exe", ["/PID", String(pid), "/T"], { stdio: "ignore" });
      } else {
        process.kill(pid, "SIGTERM");
      }
    } catch {
      // Already gone.
    }
  }
  const deadline = Date.now() + 5000;
  while (listeners(port).length) {
    if (Date.now() > deadline) {
      for (const pid of listeners(port)) {
        try {
          if (WINDOWS) {
            execFileSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
          } else {
            process.kill(pid, "SIGKILL");
          }
        } catch {
          // Already gone.
        }
      }
      await delay(200);
      break;
    }
    await delay(100);
  }
}

// 1. Build the CLI (blocking) so `dapi` and the app agree on the latest code.
console.log("[dev:desktop] building CLI…");
runNpm(["run", "build", "--workspace=@diffusionstudio/cli"]);

// 2. Start the web dev server, on a port that is free.
await reclaimPort(DEV_PORT);
console.log("[dev:desktop] starting web dev server…");
run("web", "vite", [], join(ROOT, "apps", "web"));

// 3. Once it is up, build the desktop app (blocking, mirrors its `dev`
// script) and launch Electron, which loads :5173.
try {
  await waitForServer(DEV_URL);
} catch (err) {
  console.error(`[dev:desktop] ${err.message}`);
  shutdown(1);
}
console.log("[dev:desktop] building desktop app…");
runNpm(["run", "build", "--workspace=@diffusionstudio/desktop"]);
console.log("[dev:desktop] starting desktop app…");
run("desktop", "electron-forge", ["start"], join(ROOT, "apps", "desktop"));
