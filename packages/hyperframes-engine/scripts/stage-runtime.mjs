#!/usr/bin/env node
/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve } from "node:path";
import { createRequire } from "node:module";

const HYPERFRAMES_VERSION = "0.8.30";
const CHROME_HEADLESS_SHELL_VERSION = "152.0.7977.30";
const BINARIES = process.platform === "win32" ? { ffmpeg: "ffmpeg.exe", ffprobe: "ffprobe.exe", chrome: "chrome-headless-shell.exe" } : { ffmpeg: "ffmpeg", ffprobe: "ffprobe", chrome: "chrome-headless-shell" };

function argument(name) {
  const at = process.argv.indexOf(name);
  return at === -1 ? undefined : process.argv.at(at + 1);
}

function walk(directory) {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) files.push(...walk(path));
    else files.push(path);
  }
  return files;
}

function copyLicense(packageRoot, target, name) {
  for (const license of readdirSync(packageRoot).filter((file) => /^(license|copying|notice)(\..*)?$/i.test(file))) {
    cpSync(join(packageRoot, license), join(target, `${name}-${license}`));
  }
}
function npmCliPath() {
  const candidates = [
    process.env.npm_execpath,
    join(dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js"),
    join(dirname(dirname(process.execPath)), "lib", "node_modules", "npm", "bin", "npm-cli.js"),
  ].filter(Boolean);
  const npmCli = candidates.find((path) => existsSync(path));
  if (!npmCli) throw new Error("Could not resolve npm's JavaScript CLI; run this script from npm or a standard Node installation.");
  return npmCli;
}

const outputArgument = argument("--output");
if (!outputArgument) throw new Error("Usage: node stage-runtime.mjs --output <directory> [--platform <platform>] [--arch <arch>]");
const targetPlatform = argument("--platform") ?? process.platform;
const targetArch = argument("--arch") ?? process.arch;
if (targetPlatform !== process.platform || targetArch !== process.arch) {
  throw new Error(`Runtime staging must run natively for ${targetPlatform}-${targetArch}; it installs platform-specific npm binaries and Chrome for Testing.`);
}
if (!["win32", "darwin"].includes(targetPlatform) || !["x64", "arm64"].includes(targetArch)) {
  throw new Error(`Unsupported HyperFrames runtime target: ${targetPlatform}-${targetArch}.`);
}
const output = resolve(outputArgument);
const work = mkdtempSync(join(tmpdir(), "ad3-hyperframes-stage-"));
try {
  const manifest = {
    private: true,
    type: "module",
    dependencies: {
      "@puppeteer/browsers": "3.2.1",
      "ffmpeg-static": "5.2.0",
      "@ffprobe-installer/ffprobe": "2.1.2",
      hyperframes: HYPERFRAMES_VERSION,
    },
  };
  writeFileSync(join(work, "package.json"), `${JSON.stringify(manifest, null, 2)}\n`);
  // Scripts are required here because ffmpeg-static downloads its platform-native binary
  // during installation. This staging operation is intentionally host-native.
  execFileSync(process.execPath, [npmCliPath(), "install", "--omit=dev", "--no-audit", "--no-fund"], { cwd: work, stdio: "inherit", shell: false });

  rmSync(output, { recursive: true, force: true });
  mkdirSync(output, { recursive: true });
  // npm launchers must remain relative when the temporary install tree is removed.
  cpSync(join(work, "node_modules"), join(output, "node_modules"), { recursive: true, verbatimSymlinks: true });
  cpSync(join(work, "package.json"), join(output, "package.json"));
  cpSync(join(work, "package-lock.json"), join(output, "package-lock.json"));
  const requireFromRuntime = createRequire(join(work, "runtime-resolver.cjs"));
  const hyperframesCli = requireFromRuntime.resolve("hyperframes/bin/hyperframes.mjs");
  if (!existsSync(hyperframesCli)) throw new Error("Pinned HyperFrames package is incomplete after installation.");

  const cache = join(work, "chrome-cache");
  const browserCli = join(dirname(requireFromRuntime.resolve("@puppeteer/browsers")), "main-cli.js");
  execFileSync(process.execPath, [browserCli, "install", `chrome-headless-shell@${CHROME_HEADLESS_SHELL_VERSION}`, "--path", cache], { cwd: work, stdio: "inherit", shell: false });
  const chrome = walk(cache).find((file) => basename(file) === BINARIES.chrome);
  if (!chrome) throw new Error("Chrome for Testing install completed without the pinned headless-shell executable.");
  const browserRoot = dirname(chrome);
  cpSync(browserRoot, join(output, "browser"), { recursive: true, dereference: true });
  const stagedBrowser = join("browser", relative(browserRoot, chrome));

  const ffmpegModule = requireFromRuntime("ffmpeg-static");
  const ffprobeModule = requireFromRuntime("@ffprobe-installer/ffprobe");
  const ffmpegSource = typeof ffmpegModule === "string" ? ffmpegModule : ffmpegModule?.default ?? ffmpegModule?.path;
  const ffprobeSource = typeof ffprobeModule === "string" ? ffprobeModule : ffprobeModule?.default?.path ?? ffprobeModule?.path;
  if (!ffmpegSource || !ffprobeSource || !existsSync(ffmpegSource) || !existsSync(ffprobeSource)) throw new Error("Static FFmpeg packages did not provide both required binaries.");
  mkdirSync(join(output, "ffmpeg"));
  mkdirSync(join(output, "ffprobe"));
  cpSync(ffmpegSource, join(output, "ffmpeg", BINARIES.ffmpeg));
  cpSync(ffprobeSource, join(output, "ffprobe", BINARIES.ffprobe));
  if (targetPlatform === "darwin") {
    const expected = targetArch === "arm64" ? "arm64" : "x86_64";
    for (const executable of [chrome, ffmpegSource, ffprobeSource]) {
      const architectures = execFileSync("/usr/bin/lipo", ["-archs", executable], { encoding: "utf8" }).trim().split(/\s+/);
      if (!architectures.includes(expected)) throw new Error(`${basename(executable)} is not native ${expected}: ${architectures.join(", ")}.`);
    }
  }

  const licenses = join(output, "licenses");
  mkdirSync(licenses);
  for (const [name, packageName] of [["hyperframes", "hyperframes/package.json"], ["puppeteer-browsers", "@puppeteer/browsers/package.json"], ["ffmpeg-static", "ffmpeg-static/package.json"], ["ffprobe-installer", "@ffprobe-installer/ffprobe/package.json"]]) {
    copyLicense(dirname(requireFromRuntime.resolve(packageName)), licenses, name);
  }
  cpSync(join(dirname(ffprobeSource), "package.json"), join(licenses, "FFprobe-distribution.json"));
  cpSync(new URL("../licenses/Apache-2.0.txt", import.meta.url), join(licenses, "Apache-2.0.txt"));
  cpSync(join(dirname(requireFromRuntime.resolve("ffmpeg-static/package.json")), "LICENSE"), join(licenses, "GPL-3.0.txt"));
  for (const extension of [".LICENSE", ".README"]) {
    if (existsSync(ffmpegSource + extension)) cpSync(ffmpegSource + extension, join(licenses, "FFmpeg-build" + extension));
  }
  for (const [name, executable] of [["FFmpeg", ffmpegSource], ["FFprobe", ffprobeSource]]) {
    const license = execFileSync(executable, ["-L"], { encoding: "utf8" });
    const version = execFileSync(executable, ["-version"], { encoding: "utf8" });
    writeFileSync(join(licenses, `${name}-binary.txt`), `${license}\n${version}`);
  }
  writeFileSync(join(output, "THIRD_PARTY_NOTICES.txt"), `HyperFrames runtime third-party notices

HyperFrames ${HYPERFRAMES_VERSION}: Apache-2.0 (licenses/Apache-2.0.txt).
Source: https://github.com/heygen-com/hyperframes
The bundled package.json and package-lock.json record this runtime's dependencies.
Their original source, license and copyright notices remain in node_modules.

Built-in AD3 compositions use the browser's native Web Animations API.
AD3 does not bundle or require GSAP. Custom composition authors supply and license their own dependencies.

Chrome Headless Shell ${CHROME_HEADLESS_SHELL_VERSION}: Chrome for Testing distribution.
Terms: https://www.google.com/chrome/terms/
Source: https://chromium.googlesource.com/chromium/src/
Downloaded through @puppeteer/browsers from Chrome for Testing's official endpoint.
Chrome's own notices remain in the browser directory.

FFmpeg and FFprobe binaries: see licenses/FFmpeg-binary.txt and
licenses/FFprobe-binary.txt for the actual build versions, configurations
and license terms. Full GPL-3.0 text is included in licenses/GPL-3.0.txt.
The npm wrapper's license is not a replacement for the binary's license.
FFmpeg build source reference and configuration: licenses/FFmpeg-build.README.
FFmpeg source releases: https://ffmpeg.org/releases/
FFmpeg binary distribution and build sources: https://github.com/eugeneware/ffmpeg-static
FFprobe binary distribution and platform metadata: licenses/FFprobe-distribution.json.
FFprobe installer sources: https://github.com/SavageCore/node-ffprobe-installer
`);
  const runtime = {
    version: HYPERFRAMES_VERSION,
    cli: "node_modules/hyperframes/bin/hyperframes.mjs",
    browser: stagedBrowser.split("\\").join("/"),
    ffmpeg: `ffmpeg/${BINARIES.ffmpeg}`,
    ffprobe: `ffprobe/${BINARIES.ffprobe}`,
  };
  writeFileSync(join(output, "runtime.json"), `${JSON.stringify(runtime, null, 2)}\n`);
  for (const path of [runtime.cli, runtime.browser, runtime.ffmpeg, runtime.ffprobe]) if (!statSync(join(output, path)).isFile()) throw new Error(`Staged runtime file is missing: ${path}`);
  console.log(`Staged HyperFrames ${HYPERFRAMES_VERSION} for ${targetPlatform}-${targetArch} at ${output}`);
} finally {
  rmSync(work, { recursive: true, force: true });
}
