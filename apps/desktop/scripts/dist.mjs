/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { packager } from "@electron/packager";
import { execFileSync } from "node:child_process";
import { readFile, mkdir, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { createWindowsInstaller } = require("electron-winstaller");

const desktopDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const rootDir = join(desktopDir, "..", "..");
const outDir = join(desktopDir, "out");
// Keep the executable and Squirrel package name stable so existing installs,
// taskbar pins, and the dapi launcher continue to work after the rebrand.
const executableName = "Diffusion Studio";
const productName = "AD3 Editing";
const makerName = "DiffusionStudio";

function runForge(mode) {
  const executable = join(rootDir, "node_modules", ".bin", "electron-forge");
  execFileSync(executable, [mode], { cwd: desktopDir, stdio: "inherit" });
}

function ignored(path) {
  return (
    path !== "" &&
    path !== "/package.json" &&
    path !== "/assets" &&
    path !== "/assets/icon.ico" &&
    path !== "/dist" &&
    !path.startsWith("/dist/") &&
    path !== "/web" &&
    !path.startsWith("/web/")
  );
}

async function packageWindows(version, arch) {
  await rm(outDir, { recursive: true, force: true });
  const electronVersion = require("electron/package.json").version;
  const paths = await packager({
    dir: desktopDir,
    out: outDir,
    overwrite: true,
    name: executableName,
    appVersion: version,
    appCopyright: "Copyright (c) 2026 AD3 Editing contributors. Includes MPL-2.0 components.",
    electronVersion,
    platform: "win32",
    arch,
    asar: false,
    prune: false,
    ignore: ignored,
    icon: join(desktopDir, "assets", "icon.ico"),
    extraResource: [join(desktopDir, "cli"), join(desktopDir, "docs")],
    win32metadata: {
      CompanyName: "AD3 Editing",
      FileDescription: "AD3 Editing — local professional video editor",
      InternalName: makerName,
      OriginalFilename: `${executableName}.exe`,
      ProductName: productName,
    },
  });
  const appDir = paths[0];
  if (!appDir) throw new Error("Electron Packager did not return a Windows application directory.");
  return appDir;
}

async function makeWindows(appDir, version, arch) {
  const makeDir = join(outDir, "make");
  const squirrelDir = join(makeDir, "squirrel.windows", arch);
  const zipDir = join(makeDir, "zip", "win32", arch);
  await mkdir(squirrelDir, { recursive: true });
  await mkdir(zipDir, { recursive: true });

  const setupName = `AD3-Editing-${arch}-Setup.exe`;
  await createWindowsInstaller({
    appDirectory: appDir,
    outputDirectory: squirrelDir,
    authors: "AD3 Editing",
    description: "AD3 Editing — local professional video editor",
    copyright: `Copyright © ${new Date().getFullYear()} AD3 Editing`,
    exe: `${executableName}.exe`,
    name: makerName,
    title: productName,
    version,
    setupExe: setupName,
    setupIcon: join(desktopDir, "assets", "icon.ico"),
    noMsi: true,
    usePackageJson: false,
  });

  const zipPath = join(zipDir, `AD3-Editing-${arch}-${version}.zip`);
  execFileSync(
    "tar.exe",
    ["-a", "-c", "-f", zipPath, "-C", dirname(appDir), basename(appDir)],
    { stdio: "inherit" },
  );

  return { setupPath: join(squirrelDir, setupName), zipPath };
}

async function main() {
  const mode = process.argv[2];
  if (mode !== "package" && mode !== "make") {
    throw new Error(`Expected distribution mode package or make; received ${mode ?? "nothing"}.`);
  }
  if (process.platform !== "win32") {
    runForge(mode);
    return;
  }

  const rootPackage = JSON.parse(await readFile(join(rootDir, "package.json"), "utf8"));
  const arch = process.env.npm_config_arch ?? process.arch;
  if (!new Set(["x64", "arm64", "ia32"]).has(arch)) {
    throw new Error(`Unsupported Windows architecture: ${arch}`);
  }

  const appDir = await packageWindows(rootPackage.version, arch);
  if (mode === "package") {
    console.log(`Packaged Windows application: ${appDir}`);
    return;
  }

  const artifacts = await makeWindows(appDir, rootPackage.version, arch);
  console.log(`Packaged Windows application: ${appDir}`);
  console.log(`Windows installer: ${artifacts.setupPath}`);
  console.log(`Portable Windows archive: ${artifacts.zipPath}`);
}

await main();
