/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Stages the dapi CLI into apps/desktop/cli so electron-forge can ship it as
// an application resource. The staged layout:
//   cli/dapi.js        bundled CLI (built by apps/cli)
//   cli/node_modules   deps the bundle keeps external (esbuild, babel)
//   cli/bin/dapi       POSIX wrapper
//   cli/bin/dapi.cmd   Windows wrapper
import { execFileSync } from "node:child_process";
import { chmodSync, cpSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const desktopDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const cliDir = join(desktopDir, "..", "cli");
const stageDir = join(desktopDir, "cli");

// Must match the --external list in the apps/cli build script, minus ws's
// optional native addons (bufferutil, utf-8-validate): those are external
// only so the bundle doesn't choke on them, and ws falls back to its JS
// implementations when they are absent.
const EXTERNALS = ["esbuild", "@babel/core", "@babel/preset-typescript", "babel-preset-solid"];

const cliPkg = JSON.parse(readFileSync(join(cliDir, "package.json"), "utf8"));

rmSync(stageDir, { recursive: true, force: true });
mkdirSync(join(stageDir, "bin"), { recursive: true });

cpSync(join(cliDir, "dist", "index.js"), join(stageDir, "dapi.js"));

writeFileSync(
  join(stageDir, "package.json"),
  JSON.stringify(
    {
      name: "dapi-runtime",
      private: true,
      dependencies: Object.fromEntries(EXTERNALS.map((name) => [name, cliPkg.dependencies[name]])),
    },
    null,
    2,
  ),
);

const npmExecutable = process.platform === "win32" ? (process.env.ComSpec ?? "cmd.exe") : "npm";
const npmArgs =
  process.platform === "win32"
    ? ["/d", "/s", "/c", "npm install --omit=dev --no-audit --no-fund --no-package-lock"]
    : ["install", "--omit=dev", "--no-audit", "--no-fund", "--no-package-lock"];
execFileSync(npmExecutable, npmArgs, {
  cwd: stageDir,
  stdio: "inherit",
});

// The wrappers run the CLI bundle on the app's own Electron binary in Node
// mode, so users need no separate Node installation.
const posixWrapper = `#!/bin/sh
SELF="$0"
while [ -L "$SELF" ]; do
  LINK="$(readlink "$SELF")"
  case "$LINK" in
    /*) SELF="$LINK" ;;
    *) SELF="$(dirname "$SELF")/$LINK" ;;
  esac
done
DIR="$(cd "$(dirname "$SELF")" && pwd)"
export DIFFUSION_APP_PATH="$(cd "$DIR/../../../.." && pwd)"
ELECTRON_RUN_AS_NODE=1 exec "$DIR/../../../MacOS/AD3 Editing" "$DIR/../dapi.js" "$@"
`;
writeFileSync(join(stageDir, "bin", "dapi"), posixWrapper);
if (process.platform !== "win32") chmodSync(join(stageDir, "bin", "dapi"), 0o755);

// The same dapi.cmd works in a portable package and when copied to the stable
// per-user bin directory by the Windows application menu. Squirrel keeps app
// versions in app-* directories, so the copied shim chooses the newest one.
const windowsWrapper = `@echo off
setlocal
set "APP_DIR=%~dp0..\\..\\.."
if exist "%APP_DIR%\\Diffusion Studio.exe" goto app_found

set "INSTALL_ROOT=%LOCALAPPDATA%\\DiffusionStudio"
if not exist "%INSTALL_ROOT%" goto app_missing
pushd "%INSTALL_ROOT%" >nul
for /f "delims=" %%D in ('dir /b /ad /o-d app-* 2^>nul') do (
  if exist "%%D\\Diffusion Studio.exe" (
    set "APP_DIR=%INSTALL_ROOT%\\%%D"
    goto installed_app_found
  )
)
popd
goto app_missing

:installed_app_found
popd
:app_found
set "DIFFUSION_APP_PATH=%APP_DIR%"
set "ELECTRON_RUN_AS_NODE=1"
"%APP_DIR%\\Diffusion Studio.exe" "%APP_DIR%\\resources\\cli\\dapi.js" %*
set "DAPI_EXIT=%ERRORLEVEL%"
endlocal & exit /b %DAPI_EXIT%

:app_missing
echo AD3 Editing is not installed or the portable app cannot be located. 1>&2
endlocal & exit /b 1
`;
writeFileSync(join(stageDir, "bin", "dapi.cmd"), windowsWrapper);

// Mach-O files inside Resources are not reached by the app-bundle signing
// pass, and notarization rejects unsigned executables; sign them here.
if (process.platform === "darwin" && !process.env.SKIP_SIGN) {
  const identities = execFileSync("security", ["find-identity", "-v", "-p", "codesigning"], {
    encoding: "utf8",
  });
  const identity = identities.match(/"(Developer ID Application: [^"]+)"/)?.[1];
  if (identity) {
    const esbuildDir = join(stageDir, "node_modules", "@esbuild");
    for (const pkg of readdirSync(esbuildDir)) {
      const bin = join(esbuildDir, pkg, "bin", "esbuild");
      execFileSync("codesign", ["--force", "--options", "runtime", "--timestamp", "--sign", identity, bin], {
        stdio: "inherit",
      });
    }
  } else {
    console.warn("stage-cli: no Developer ID identity found, leaving esbuild binary unsigned");
  }
}

console.log(`stage-cli: staged dapi at ${stageDir}`);
