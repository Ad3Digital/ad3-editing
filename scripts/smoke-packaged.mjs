/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Release smoke: runs the packaged application and its packaged HyperFrames
// runtime. All project/profile/output bytes stay below --output.
import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { createServer } from "node:net";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { parseArgs, promisify } from "node:util";

const { values } = parseArgs({ options: {
  app: { type: "string" }, output: { type: "string" }, attach: { type: "string" },
} });
if (!values.app || !values.output) throw new Error("Usage: node scripts/smoke-packaged.mjs --app <executable> --output <proof directory> [--attach <CDP URL>]");
const app = resolve(values.app);
const output = resolve(values.output);
const resources = process.platform === "darwin" ? resolve(dirname(app), "..", "Resources") : join(dirname(app), "resources");
const runtime = join(resources, "hyperframes-engine");
const manifest = JSON.parse(await readFile(join(runtime, "runtime.json"), "utf8"));
const requireRuntime = createRequire(join(runtime, "package.json"));
const { default: puppeteer } = await import(pathToFileURL(requireRuntime.resolve("puppeteer-core")).href);
const requireDesktop = createRequire(new URL("../apps/desktop/package.json", import.meta.url));
const { parse: parseYaml } = requireDesktop("yaml");
const exec = promisify(execFile);
await mkdir(output, { recursive: true });
const profile = join(output, "profile");
let child;
let browser;
let appLog = "";
const errors = [];
const proof = { platform: process.platform, arch: process.arch, hyperframes: manifest.version, checks: [] };
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

async function cli(...args) {
  const { stdout } = await exec(app, [join(resources, "cli", "dapi.js"), ...args], {
    env: { ...process.env, ELECTRON_RUN_AS_NODE: "1" }, timeout: 120000,
  });
  return JSON.parse(stdout);
}

async function until(run, message, timeout = 120000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const result = await run();
    if (result) return result;
    if (child?.exitCode !== null && child?.exitCode !== undefined) throw new Error(`Application exited (${child.exitCode}): ${appLog.slice(-4000)}`);
    await sleep(250);
  }
  throw new Error(message);
}

async function availablePort() {
  const server = createServer();
  await new Promise((done, fail) => { server.once("error", fail); server.listen(0, "127.0.0.1", done); });
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return port;
}

async function call(page, channel, data) {
  return page.evaluate((channel, data) => new Promise((done, fail) => {
    const id = crypto.randomUUID();
    const timer = setTimeout(() => { off(); fail(new Error(`IPC timeout: ${channel}`)); }, 120000);
    const off = window.desktop.on("main:response", (reply) => {
      if (reply.id !== id) return;
      clearTimeout(timer); off();
      if (reply.ok) done(reply.data); else fail(new Error(reply.error));
    });
    window.desktop.send("main:request", { id, channel, data });
  }), channel, data);
}

async function clickButton(page, pattern) {
  const found = await page.evaluate((source) => {
    const pattern = new RegExp(source, "i");
    const button = [...document.querySelectorAll("button")].find((element) =>
      !element.disabled && pattern.test(element.textContent.trim()) && element.getBoundingClientRect().width > 0);
    if (!button) return false;
    button.click(); return true;
  }, pattern);
  assert(found, `Visible enabled button not found: ${pattern}`);
}

function source(title, transparent = false) {
  return `<!doctype html><html><head><meta charset="utf-8"><style>
html,body{margin:0;width:320px;height:180px;overflow:hidden}#smoke{position:relative;width:320px;height:180px;color:#fff;font-family:Arial,sans-serif}#fill{position:absolute;inset:0;background:#18324c}#heading{position:absolute;left:24px;top:64px;font-size:24px;margin:0}
</style></head><body><div id="smoke" data-composition-id="smoke" data-width="320" data-height="180" data-start="0" data-duration="1.5">
${transparent ? "" : '<div id="fill"></div>'}<div class="clip" data-start="0" data-duration="1.5" data-track-index="0"><h1 id="heading">${title}</h1></div></div>
<script>const motion=document.querySelector('#heading').animate([{opacity:0,transform:'translateX(-12px)'},{opacity:1,transform:'translateX(0)',offset:0.2},{opacity:1,transform:'translateX(0)',offset:0.4667},{opacity:1,transform:'translateX(16px)',offset:0.8},{opacity:1,transform:'translateX(16px)'}],{duration:1500,iterations:1,fill:'both',easing:'linear'});motion.pause();</script></body></html>`;
}

async function waitJob(page, dir, job) {
  return until(async () => {
    const next = await call(page, "hyperframes:job", { dir, jobId: job.id });
    if (next.state === "failed") throw new Error(next.error + "\n" + next.log);
    if (next.state === "cancelled") throw new Error("Render unexpectedly cancelled");
    return next.state === "completed" ? next : null;
  }, "HyperFrames render did not complete", 600000);
}

try {
  let endpoint = values.attach;
  if (!endpoint) {
    const port = await availablePort();
    endpoint = `http://127.0.0.1:${port}`;
    const args = [`--user-data-dir=${profile}`, `--remote-debugging-port=${port}`, "--remote-debugging-address=127.0.0.1"];
    if (process.env.CI) args.push("--enable-unsafe-webgpu", "--use-angle=swiftshader");
    child = spawn(app, args, { stdio: ["ignore", "pipe", "pipe"] });
    child.stdout.on("data", (data) => { appLog = (appLog + data).slice(-200000); });
    child.stderr.on("data", (data) => { appLog = (appLog + data).slice(-200000); });
    child.on("error", (error) => { appLog += error.message; });
  }
  await until(async () => { try { return (await fetch(endpoint + "/json/version")).ok; } catch { return false; } }, "Packaged Electron did not expose its local CDP endpoint");
  browser = await puppeteer.connect({ browserURL: endpoint, defaultViewport: null });
  const page = await until(async () => (await browser.pages()).find((page) => page.url().startsWith("file:")), "Packaged application page did not open");
  page.on("pageerror", (error) => errors.push(error.message));
  await page.waitForFunction(() => !!window.desktop && document.readyState === "complete", { timeout: 120000 });
  await page.screenshot({ path: join(output, "dashboard.png") });
  const status = await call(page, "hyperframes:status");
  assert.equal(status.available, true, status.reason);
  assert.equal(status.version, manifest.version);
  proof.checks.push("Packaged local engine available without a system Node/Chrome/FFmpeg install");

  const project = await call(page, "projects:create", { root: join(output, "projects"), displayName: "HyperFrames release smoke" });
  const dir = project.dir;
  await writeFile(join(dir, project.entry), '/* @jsxImportSource @diffusionstudio/jsx */\nexport default function Smoke() { return <stage><scene id="hf-smoke-scene" name="HyperFrames smoke" width={320} height={180} fill="#18324c" active /></stage>; }\n');
  const settings = { template: "custom", title: "First render", subtitle: "", accent: "#5eead4", background: "#18324c", duration: 1.5, width: 320, height: 180, fps: 24, transparent: false };
  let composition = await call(page, "hyperframes:save", { dir, draft: { name: "Release smoke", settings, html: source("First render") } });
  const saved = await call(page, "hyperframes:list", { dir });
  assert.equal(saved.find((entry) => entry.id === composition.id)?.html, composition.html);
  const preview = await call(page, "hyperframes:preview", { dir, id: composition.id });
  const previewUrl = new URL(preview.url);
  assert(["127.0.0.1", "localhost", "[::1]"].includes(previewUrl.hostname));
  const previewBrowser = await puppeteer.launch({
    executablePath: join(runtime, manifest.browser),
    userDataDir: join(output, "preview-profile"),
    headless: true,
  });
  try {
    const previewPage = await previewBrowser.newPage();
    await previewPage.goto(preview.url, { waitUntil: "networkidle2", timeout: 120000 });
    await previewPage.waitForFunction(() => document.querySelector("hyperframes-player")?.ready, { timeout: 120000 });
    assert.equal(await previewPage.$eval("hyperframes-player", (player) => player.duration), 1.5);
    await previewPage.$eval("hyperframes-player", (player) => player.seek(0.7));
    await previewPage.waitForFunction(() => {
      const frame = document.querySelector("hyperframes-player")?.iframeElement;
      const heading = frame?.contentDocument?.querySelector("#heading");
      return heading && Number(frame.contentWindow.getComputedStyle(heading).opacity) > 0.9;
    }, { timeout: 120000 });
    await previewPage.screenshot({ path: join(output, "hyperframes-preview.png") });
    assert.equal(await previewPage.evaluate(() => typeof window.desktop), "undefined");
  } finally {
    await previewBrowser.close();
  }
  proof.checks.push("Saved source reopened; native preview player loaded the composition and visibly sought to 0.7 seconds");

  let job = await waitJob(page, dir, await call(page, "hyperframes:render", { dir, id: composition.id }));
  const firstSource = job.result.source;
  const { stdout: probeText } = await exec(join(runtime, manifest.ffprobe), ["-v", "error", "-show_streams", "-show_format", "-of", "json", join(dir, firstSource)]);
  const probe = JSON.parse(probeText);
  const video = probe.streams.find((stream) => stream.codec_type === "video");
  assert.equal(video.width, 320); assert.equal(video.height, 180);
  assert(Math.abs(Number(probe.format.duration) - 1.5) < 0.1);
  proof.mp4 = { width: video.width, height: video.height, duration: Number(probe.format.duration), codec: video.codec_name };
  await exec(join(runtime, manifest.ffmpeg), ["-hide_banner", "-loglevel", "error", "-y", "-ss", "0.7", "-i", join(dir, firstSource), "-frames:v", "1", join(output, "rendered-first.png")]);
  proof.checks.push("Actual bundled HyperFrames render produced a decodable 320x180, 1.5-second video");

  // Seed only the isolated profile's normal recent-project record, then drive
  // the real project route and panel. No user profile or user project is used.
  await page.evaluate((project) => new Promise((done, fail) => {
    const request = indexedDB.open("diffusion-studio-idb", 2);
    request.onerror = () => fail(request.error);
    request.onsuccess = () => {
      const db = request.result;
      const tx = db.transaction("roots", "readwrite");
      const now = new Date().toISOString();
      tx.objectStore("roots").put({ id: crypto.randomUUID(), path: project.dir, name: project.displayName, kind: "single", createdAt: now, lastUsedAt: now });
      tx.oncomplete = () => { db.close(); done(); };
      tx.onerror = () => fail(tx.error);
    };
  }), project);
  await page.goto(page.url().split("#")[0] + "#/projects/" + encodeURIComponent(project.id));
  await page.waitForFunction(() => [...document.querySelectorAll("button")].some((button) => /hyperframes/i.test(button.textContent)), { timeout: 120000 });
  await page.bringToFront();
  await clickButton(page, "^HyperFrames$");
  await page.waitForFunction(() => document.body.innerText.includes("Release smoke"), { timeout: 120000 });
  await clickButton(page, "^Release smoke$");
  await page.waitForFunction(() => [...document.querySelectorAll("button")].some((button) => button.textContent.trim() === "Insert at playhead"), { timeout: 120000 });
  await clickButton(page, "^Preview$");
  await page.waitForSelector('iframe[title="HyperFrames preview"]', { timeout: 120000 });
  const frameElement = await page.$('iframe[title="HyperFrames preview"]');
  await frameElement.scrollIntoView();
  const panelPreviewUrl = await frameElement.evaluate((element) => element.src);
  const previewFrame = await until(() => page.frames().find((frame) => frame.url() === panelPreviewUrl), "Preview player frame did not navigate");
  await previewFrame.waitForSelector('pierce/button[aria-label="Play"]', { timeout: 120000 });
  const compositionFrame = await until(() => page.frames().find((frame) => frame.parentFrame() === previewFrame && frame.url().endsWith("/preview")), "Preview composition iframe did not load");
  await previewFrame.click('pierce/button[aria-label="Play"]');
  await compositionFrame.waitForFunction(() => {
    const heading = document.querySelector("#heading");
    return heading?.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }) && Number(getComputedStyle(heading).opacity) > 0.95;
  }, { timeout: 120000 });
  await previewFrame.click('pierce/button[aria-label="Pause"]');
  assert.equal(await compositionFrame.evaluate(() => typeof window.desktop), "undefined");
  assert.equal(await previewFrame.evaluate(() => typeof window.desktop), "undefined");
  await page.screenshot({ path: join(output, "hyperframes-panel.png") });
  proof.checks.push("Panel preview played and paused visible animation without exposing the desktop bridge");
  await clickButton(page, "insert.*(timeline|clip)|insert at playhead");
  const manifestPath = join(dir, "assets.yml");
  const linkedRecord = async () => parseYaml(await readFile(manifestPath, "utf8"))?.assets?.find((asset) => asset.generation?.key === "hyperframes" && asset.generation?.id === composition.id);
  const originalRecord = await until(linkedRecord, "Rendered composition was not registered as generated media");
  const linkedBefore = await until(async () => {
    const context = await cli("context");
    assert.equal(resolve(context.projectDir), resolve(dir));
    return context.generations.some((row) => row.asset === originalRecord.path && row.state === "done") ? context.generations : null;
  }, "Inserted clip did not resolve to the generated media");
  assert.equal(linkedBefore.length, 1);
  proof.checks.push("Panel inserted the rendered composition as a linked timeline asset");

  await page.$eval('textarea[aria-label="HyperFrames HTML source"]', (element, html) => {
    element.value = html;
    element.dispatchEvent(new Event("input", { bubbles: true }));
  }, source("Updated render"));
  await clickButton(page, "^Render$");
  const updated = await until(async () => {
    const problem = await page.$eval('[role="alert"]', (element) => element.textContent).catch(() => "");
    if (problem) throw new Error(problem);
    const compositions = await call(page, "hyperframes:list", { dir });
    const rendered = compositions.find((item) => item.id === composition.id)?.rendered;
    return rendered && rendered.source !== firstSource ? rendered : null;
  }, "Panel render did not produce the edited composition", 600000);
  await until(async () => {
    const asset = await linkedRecord();
    return asset?.source === updated.source && asset.path === originalRecord.path;
  }, "Re-render did not preserve the linked asset's library path");
  const linkedAfter = await cli("context");
  assert.deepEqual(linkedAfter.generations, linkedBefore, "Re-render must preserve the linked timeline element and logical asset");
  const captured = await cli("capture", "hf-smoke-scene", "-t", "0.7", "--separate", "-o", output);
  assert.equal((await readFile(captured.path)).subarray(1, 4).toString(), "PNG");
  await exec(join(runtime, manifest.ffmpeg), ["-hide_banner", "-loglevel", "error", "-y", "-ss", "0.7", "-i", join(dir, updated.source), "-frames:v", "1", join(output, "rendered-updated.png")]);
  await page.screenshot({ path: join(output, "hyperframes-updated.png") });
  proof.checks.push("Re-render produced an immutable output and the panel updated the linked asset");

  const transparent = await call(page, "hyperframes:save", { dir, draft: { name: "Transparent smoke", settings: { ...settings, transparent: true }, html: source("Transparent", true) } });
  const alphaJob = await waitJob(page, dir, await call(page, "hyperframes:render", { dir, id: transparent.id }));
  assert.equal(alphaJob.result.format, "png-sequence");
  const { readdir } = await import("node:fs/promises");
  const pngDir = join(dir, alphaJob.result.source);
  const frames = (await readdir(pngDir)).filter((name) => /\.png$/i.test(name)).sort();
  assert.equal(frames.length, 36);
  const { stdout: alphaText } = await exec(join(runtime, manifest.ffprobe), ["-v", "error", "-show_streams", "-of", "json", join(pngDir, frames[12])]);
  assert.match(JSON.parse(alphaText).streams[0].pix_fmt, /rgba|yuva|gbrap/);
  const { stdout: alphaPixels } = await exec(join(runtime, manifest.ffmpeg), ["-hide_banner", "-loglevel", "error", "-i", join(pngDir, frames[12]), "-vf", "alphaextract", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "gray", "pipe:1"], { encoding: "buffer" });
  assert(alphaPixels.includes(0) && alphaPixels.includes(255), "Transparent render must contain both clear background and opaque text");
  proof.checks.push("Transparent render preserved RGBA and exact 24-fps frame count");
  await exec(join(runtime, manifest.ffmpeg), ["-hide_banner", "-loglevel", "error", "-y", "-i", join(pngDir, frames[12]), "-frames:v", "1", join(output, "rendered-transparent.png")]);

  const otherProject = await call(page, "projects:create", { root: join(output, "projects"), displayName: "Separate project" });
  const cancelJob = await call(page, "hyperframes:render", { dir, id: composition.id });
  await assert.rejects(() => call(page, "hyperframes:job", { dir: otherProject.dir, jobId: cancelJob.id }), /project|unknown/i);
  const cancelled = await call(page, "hyperframes:cancel", { dir, jobId: cancelJob.id });
  assert.equal(cancelled.state, "cancelled");
  await sleep(1000);
  assert.equal((await call(page, "hyperframes:job", { dir, jobId: cancelJob.id })).state, "cancelled");
  await assert.rejects(() => call(page, "hyperframes:save", { dir, draft: { id: "../escape", name: "Invalid", settings, html: source("Invalid") } }), /id|unsafe|invalid/i);
  proof.checks.push("Cancellation remained terminal; cross-project job access and traversal were rejected");
  const remoteRequests = await page.evaluate(() => performance.getEntriesByType("resource").map((entry) => entry.name).filter((url) => /^https?:/.test(url) && !["127.0.0.1", "localhost", "[::1]"].includes(new URL(url).hostname)));
  assert.deepEqual(remoteRequests, [], "Local-only desktop mode must not load analytics or contact third-party services");
  assert.deepEqual(errors, [], "Packaged renderer emitted uncaught errors");
  proof.ok = true;
  console.log("Packaged smoke completed: " + JSON.stringify(proof));
} catch (error) {
  proof.ok = false;
  proof.error = error.stack ?? String(error);
  throw error;
} finally {
  await writeFile(join(output, "proof.json"), JSON.stringify(proof, null, 2) + "\n");
  await writeFile(join(output, "application.log"), appLog);
  await browser?.disconnect();
  if (child && child.exitCode === null) {
    if (process.platform === "win32") await exec("taskkill", ["/pid", String(child.pid), "/t", "/f"]).catch(() => {});
    else { child.kill("SIGTERM"); await Promise.race([new Promise((done) => child.once("exit", done)), sleep(10000)]); if (child.exitCode === null) child.kill("SIGKILL"); }
  }
}
