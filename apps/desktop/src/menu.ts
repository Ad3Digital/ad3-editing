/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { app, dialog, Menu } from "electron";
import { execFile } from "node:child_process";
import { copyFile, mkdir } from "node:fs/promises";
import { join } from "node:path";
import type { MenuItemConstructorOptions } from "electron";

const MACOS_CLI_LINK_PATH = "/usr/local/bin/dapi";
const WINDOWS_CLI_ROOT = "DiffusionStudio";

function run(file: string, args: string[], env?: NodeJS.ProcessEnv): Promise<void> {
  const { promise, resolve, reject } = Promise.withResolvers<void>();
  execFile(file, args, { env }, (error) => (error ? reject(error) : resolve()));
  return promise;
}

// Linking into /usr/local/bin needs elevation; osascript shows the standard
// macOS admin prompt so the app itself never asks for credentials.
async function installMacosCli(): Promise<string> {
  const wrapper = join(process.resourcesPath, "cli", "bin", "dapi");
  const shell = `mkdir -p /usr/local/bin && ln -sf '${wrapper}' '${MACOS_CLI_LINK_PATH}'`;
  const script = `do shell script "${shell.replaceAll('"', '\\"')}" with administrator privileges`;
  await run("osascript", ["-e", script]);
  return MACOS_CLI_LINK_PATH;
}

// Squirrel installs each update into a versioned app-* directory. The copied
// shim stays in a stable per-user bin directory and locates the newest app at
// runtime, so updating the desktop app cannot leave dapi pointing at old code.
async function installWindowsCli(): Promise<string> {
  const localAppData = process.env.LOCALAPPDATA;
  if (!localAppData) throw new Error("LOCALAPPDATA is not available for this Windows user.");
  const binDir = join(localAppData, WINDOWS_CLI_ROOT, "bin");
  const destination = join(binDir, "dapi.cmd");
  await mkdir(binDir, { recursive: true });
  await copyFile(join(process.resourcesPath, "cli", "bin", "dapi.cmd"), destination);

  const addToPath = [
    "$target = $env:DIFFUSION_CLI_DIR",
    "$current = [Environment]::GetEnvironmentVariable('Path', 'User')",
    "$entries = @($current -split ';' | Where-Object { $_ })",
    "if (-not ($entries | Where-Object { $_.TrimEnd([char]92) -ieq $target.TrimEnd([char]92) })) {",
    "  $next = if ([string]::IsNullOrWhiteSpace($current)) { $target } else { $current.TrimEnd(';') + ';' + $target }",
    "  [Environment]::SetEnvironmentVariable('Path', $next, 'User')",
    "}",
  ].join("\n");
  await run(
    "powershell.exe",
    ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", addToPath],
    { ...process.env, DIFFUSION_CLI_DIR: binDir },
  );
  return destination;
}

async function installCli() {
  try {
    const destination =
      process.platform === "win32" ? await installWindowsCli() : await installMacosCli();
    await dialog.showMessageBox({
      type: "info",
      message: "The AD3 Editing dapi command line tool was installed.",
      detail:
        process.platform === "win32"
          ? `Installed at ${destination} and added its folder to your user PATH. Open a new terminal, then run "dapi --help".`
          : `Linked at ${destination}. Run "dapi --help" in a terminal to get started.`,
    });
  } catch (e) {
    const message = (e as Error).message ?? "";
    if (process.platform === "darwin" && message.includes("-128")) return;
    await dialog.showMessageBox({
      type: "error",
      message: "Could not install the AD3 Editing dapi command line tool.",
      detail: message,
    });
  }
}

const cliMenuItem: MenuItemConstructorOptions = {
  label: "Install dapi Command Line Tool…",
  enabled: app.isPackaged,
  click: installCli,
};

export function setupAppMenu() {
  let template: MenuItemConstructorOptions[];
  if (process.platform === "darwin") {
    template = [
      {
        label: app.name,
        submenu: [
          { role: "about" },
          { type: "separator" },
          cliMenuItem,
          { type: "separator" },
          { role: "services" },
          { type: "separator" },
          { role: "hide" },
          { role: "hideOthers" },
          { role: "unhide" },
          { type: "separator" },
          { role: "quit" },
        ],
      },
      { role: "fileMenu" },
      { role: "editMenu" },
      { role: "viewMenu" },
      { role: "windowMenu" },
    ];
  } else if (process.platform === "win32") {
    template = [
      { role: "fileMenu" },
      { role: "editMenu" },
      { role: "viewMenu" },
      { label: "Tools", submenu: [cliMenuItem] },
      { role: "windowMenu" },
    ];
  } else {
    return;
  }

  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}
