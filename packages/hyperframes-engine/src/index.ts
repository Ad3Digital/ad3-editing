/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { createHash, randomUUID } from "node:crypto";
import { spawn, type ChildProcess } from "node:child_process";
import { access, copyFile, lstat, mkdir, readdir, readFile, realpath, rename, rm, stat, writeFile } from "node:fs/promises";
import { constants, realpathSync, statSync as statSyncNow } from "node:fs";
import { basename, delimiter, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { createServer } from "node:net";
import { DEFAULT_HYPERFRAMES_SETTINGS, generateTemplate } from "./templates";
import type { HyperframesComposition, HyperframesDraft, HyperframesEngineOptions, HyperframesJob, HyperframesRenderResult, HyperframesRuntimeStatus, HyperframesSettings } from "./types";

export * from "./types";
export { DEFAULT_HYPERFRAMES_SETTINGS, generateTemplate } from "./templates";

const MAX_LOG_BYTES = 16_384;
const ID = /^[a-z0-9][a-z0-9-]{0,63}$/;
const SECRET = /((?:api|access|auth|refresh|session)[_-]?token|password|secret|authorization)\s*(?:=|:)?\s*[^\s,;]+/gi;
type RuntimeLayout = { version: string; cli: string; browser: string; ffmpeg: string; ffprobe: string };
type StoredComposition = HyperframesComposition & { schema: 1; inputFingerprint?: string };
type JobRecord = { project: string; job: HyperframesJob };
type Snapshot = { dir: string; fingerprint: string };
type ActivePreview = { child: ChildProcess; ready: Promise<void>; url: string };

function inside(root: string, target: string): boolean {
  const path = relative(root, target);
  return path === "" || (!path.startsWith(`..${sep}`) && path !== ".." && !isAbsolute(path));
}
function errorText(error: unknown): string { return error instanceof Error ? error.message : String(error); }
function log(current: string, chunk: string): string {
  const next = `${current}${chunk}`.replace(SECRET, "$1=[redacted]");
  return next.length <= MAX_LOG_BYTES ? next : next.slice(-MAX_LOG_BYTES);
}
function revision(html: string, settings: HyperframesSettings): string {
  return createHash("sha256").update(html).update("\0").update(JSON.stringify(settings)).digest("hex").slice(0, 24);
}
function relativeProject(project: string, path: string): string {
  const result = relative(project, path).split(sep).join("/");
  if (!result || result === ".." || result.startsWith("../")) throw new Error("Engine attempted to publish a path outside the selected project.");
  return result;
}
async function present(path: string): Promise<boolean> { try { await access(path); return true; } catch { return false; } }
async function writeAtomic(path: string, content: string): Promise<void> {
  // The engine-owned revision area is excluded from source fingerprints and snapshots.
  const directory = join(dirname(path), ".revisions");
  await mkdir(directory, { recursive: true });
  if ((await lstat(directory)).isSymbolicLink() || !(await stat(directory)).isDirectory()) throw new Error("Unsafe composition write directory.");
  const temporary = join(directory, `${randomUUID()}.tmp`);
  try {
    await writeFile(temporary, content, { encoding: "utf8", flag: "wx" });
    await rename(temporary, path);
  } finally {
    await rm(temporary, { force: true });
  }
}
async function loopbackPort(): Promise<number> {
  const server = createServer();
  const listening = Promise.withResolvers<void>();
  server.once("error", listening.reject);
  server.listen(0, "127.0.0.1", listening.resolve);
  await listening.promise;
  const address = server.address();
  const closed = Promise.withResolvers<void>();
  server.close((error) => error ? closed.reject(error) : closed.resolve());
  await closed.promise;
  if (!address || typeof address === "string") throw new Error("Could not reserve a loopback preview port.");
  return address.port;
}
async function waitPort(port: number, child: ChildProcess): Promise<void> {
  const deadline = Date.now() + 30_000;
  for (;;) {
    if (child.exitCode !== null) throw new Error(`HyperFrames preview exited before becoming ready (code ${child.exitCode}).`);
    try { if ((await fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(1_000) })).status > 0) return; } catch { /* booting */ }
    if (Date.now() >= deadline) throw new Error("Timed out waiting for the local HyperFrames preview server.");
    const delay = Promise.withResolvers<void>(); setTimeout(delay.resolve, 100); await delay.promise;
  }
}

/** Node-only adapter for the staged, offline HyperFrames CLI runtime. */
export class HyperframesEngine {
  readonly #runtimeDir: string;
  readonly #nodePath: string;
  readonly #jobs = new Map<string, JobRecord>();
  readonly #children = new Map<string, ChildProcess>();
  readonly #previews = new Map<string, ActivePreview>();
  #disposed = false;

  constructor(options: HyperframesEngineOptions) { this.#runtimeDir = resolve(options.runtimeDir); this.#nodePath = resolve(options.nodePath); }

  async status(): Promise<HyperframesRuntimeStatus> {
    try {
      const runtime = await this.#runtime();
      await Promise.all([access(this.#nodePath, constants.X_OK), access(resolve(this.#runtimeDir, runtime.cli)), access(resolve(this.#runtimeDir, runtime.browser), constants.X_OK), access(resolve(this.#runtimeDir, runtime.ffmpeg), constants.X_OK), access(resolve(this.#runtimeDir, runtime.ffprobe), constants.X_OK)]);
      return { available: true, version: runtime.version };
    } catch (error) { return { available: false, version: "0.8.30", reason: errorText(error) }; }
  }

  async list(projectDir: string): Promise<HyperframesComposition[]> {
    const project = await this.#project(projectDir);
    const root = await this.#directory(project, "hyperframes", false);
    if (!(await present(root))) return [];
    const found: HyperframesComposition[] = [];
    for (const entry of await readdir(root, { withFileTypes: true })) {
      if (!entry.isDirectory() || !ID.test(entry.name)) continue;
      if (!(await present(join(root, entry.name, "composition.json")))) continue;
      found.push(await this.#composition(project, entry.name));
    }
    return found.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  }

  async save(projectDir: string, draft: HyperframesDraft): Promise<HyperframesComposition> {
    this.#live();
    const project = await this.#project(projectDir);
    const id = draft.id ?? `hf-${randomUUID().replace(/-/g, "").slice(0, 20)}`;
    if (!ID.test(id)) throw new Error("Composition IDs must use lowercase letters, digits, and hyphens only.");
    const settings = this.#settings(draft.settings);
    const html = draft.html || (settings.template === "custom" ? "" : generateTemplate(settings));
    if (!draft.name.trim() || !html.trim()) throw new Error("A HyperFrames composition needs a name and editable HTML source.");
    let prior: HyperframesComposition | undefined;
    let priorFingerprint: string | undefined;
    try {
      prior = await this.#composition(project, id);
      priorFingerprint = await this.#sourceFingerprint(await this.#directory(project, join("hyperframes", id), false));
    } catch { /* first save */ }
    const sourceRevision = revision(html, settings);
    const directory = await this.#directory(project, join("hyperframes", id), true);
    const priorStored = prior as StoredComposition | undefined;
    const preserve = prior?.revision === sourceRevision && priorStored?.inputFingerprint === priorFingerprint;
    const composition: StoredComposition = { schema: 1, id, name: draft.name.trim(), settings, html, revision: sourceRevision, updatedAt: new Date().toISOString(), rendered: preserve ? prior?.rendered : undefined, inputFingerprint: preserve ? priorStored?.inputFingerprint : undefined };
    await writeAtomic(join(directory, "index.html"), html);
    await writeAtomic(join(directory, "composition.json"), JSON.stringify(composition, null, 2));
    return composition;
  }

  async preview(projectDir: string, id: string): Promise<{ url: string }> {
    this.#live();
    const project = await this.#project(projectDir);
    const composition = await this.#composition(project, id);
    const key = `${project}\u0000${id}`;
    const prior = this.#previews.get(key);
    if (prior) await this.#kill(prior.child);
    const snapshot = await this.#snapshot(project, composition);
    const projectId = encodeURIComponent(basename(snapshot.dir));
    const compositionUrl = `/api/projects/${projectId}/preview`;
    await copyFile(
      resolve(this.#runtimeDir, "node_modules/hyperframes/dist/hyperframes-player.global.js"),
      join(snapshot.dir, "ad3-player.js"),
    );
    await writeFile(join(snapshot.dir, "ad3-preview.html"), `<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>HyperFrames preview</title><style>html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#111}hyperframes-player{display:block;width:100vw;height:100vh}</style></head><body><hyperframes-player src="${compositionUrl}" runtime-src="/api/runtime.js" controls muted></hyperframes-player><script src="./ad3-player.js"></script></body></html>`, "utf8");
    this.#live();
    const port = await loopbackPort();
    const child = await this.#spawn(project, snapshot.dir, ["preview", snapshot.dir, "--port", String(port), "--no-open", "--foreground", "--json"]);
    const url = `http://127.0.0.1:${port}${compositionUrl}/ad3-preview.html`;
    const boot = Promise.race([waitPort(port, child), this.#wait(child).then((code) => { throw new Error(`HyperFrames preview exited before becoming ready (code ${code}).`); })]);
    const preview: ActivePreview = { child, ready: boot, url };
    this.#previews.set(key, preview);
    child.once("exit", () => { if (this.#previews.get(key) === preview) this.#previews.delete(key); });
    try { await boot; return { url }; } catch (error) { await this.#kill(child); if (this.#previews.get(key) === preview) this.#previews.delete(key); throw error; }
  }

  render(projectDir: string, id: string): HyperframesJob {
    this.#live();
    const project = this.#canonical(projectDir);
    if (!ID.test(id)) throw new Error("Invalid HyperFrames composition ID.");
    const job: HyperframesJob = { id: `hf-job-${randomUUID()}`, compositionId: id, state: "running", log: "Queued HyperFrames render.\n" };
    this.#jobs.set(job.id, { project, job });
    void this.#run(project, job);
    return this.#copyJob(job);
  }

  job(projectDir: string, jobId: string): HyperframesJob {
    const record = this.#jobs.get(jobId);
    if (!record || record.project !== this.#canonical(projectDir)) throw new Error(`Unknown HyperFrames job: ${jobId}`);
    return this.#copyJob(record.job);
  }

  async cancel(projectDir: string, jobId: string): Promise<HyperframesJob> {
    const record = this.#jobs.get(jobId);
    if (!record || record.project !== this.#canonical(projectDir)) throw new Error(`Unknown HyperFrames job: ${jobId}`);
    if (record.job.state !== "running") return this.#copyJob(record.job);
    record.job.state = "cancelled"; record.job.log = log(record.job.log, "Render cancelled.\n");
    const child = this.#children.get(jobId); if (child) await this.#kill(child);
    return this.#copyJob(record.job);
  }

  async dispose(): Promise<void> {
    if (this.#disposed) return;
    this.#disposed = true;
    for (const { job } of this.#jobs.values()) if (job.state === "running") job.state = "cancelled";
    await Promise.all([...this.#children.values(), ...[...this.#previews.values()].map((preview) => preview.child)].map((child) => this.#kill(child)));
    this.#children.clear(); this.#previews.clear();
  }

  async #run(project: string, job: HyperframesJob): Promise<void> {
    let partial: string | undefined; let final: string | undefined; let metadata: { path: string; previous: string; committed: string } | undefined;
    try {
      const composition = await this.#composition(project, job.compositionId); this.#running(job);
      const snapshot = await this.#snapshot(project, composition); this.#running(job);
      // Keep incomplete and superseded outputs outside the library's automatic assets scan.
      const outputDir = await this.#directory(project, join("hyperframes-renders", composition.id, composition.revision, job.id), true); this.#running(job);
      partial = join(outputDir, composition.settings.transparent ? "partial-frames" : "partial.mp4");
      await rm(partial, { recursive: true, force: true }); this.#running(job);
      const args = ["render", snapshot.dir, "--composition", "index.html", "--output", partial, "--fps", String(composition.settings.fps), "--quality", "standard", "--quiet", "--json", ...(composition.settings.transparent ? ["--format", "png-sequence"] : ["--format", "mp4"])];
      const child = await this.#spawn(project, snapshot.dir, args, (chunk) => job.log = log(job.log, chunk));
      if (job.state !== "running" || this.#disposed) { await this.#kill(child); return; }
      this.#children.set(job.id, child); const code = await this.#wait(child); this.#children.delete(job.id); this.#running(job);
      if (code !== 0) throw new Error(`HyperFrames render failed with exit code ${code ?? "unknown"}.`);
      const probed = await this.#probe(project, partial, composition); this.#running(job);
      final = join(outputDir, composition.settings.transparent ? "frames" : "render.mp4"); await rename(partial, final); partial = undefined; this.#running(job);
      const current = await this.#sourceFingerprint(await this.#directory(project, join("hyperframes", composition.id), false));
      if (current !== snapshot.fingerprint) throw new Error("Composition assets changed while rendering; output was not attached.");
      const saved = await this.#composition(project, composition.id);
      if (saved.revision !== composition.revision) throw new Error("Composition changed while rendering; output was not attached.");
      this.#running(job);
      const result = { ...probed, source: relativeProject(project, final) };
      const sourceDir = await this.#directory(project, join("hyperframes", composition.id), false);
      const metadataPath = join(sourceDir, "composition.json"); const previous = await readFile(metadataPath, "utf8");
      const committed = JSON.stringify({ ...saved, rendered: result, inputFingerprint: snapshot.fingerprint }, null, 2);
      await writeAtomic(metadataPath, committed); metadata = { path: metadataPath, previous, committed };
      this.#running(job); job.state = "completed"; job.result = result; job.log = log(job.log, "Render completed.\n");
    } catch (error) {
      if (metadata && job.state !== "completed" && (await readFile(metadata.path, "utf8")) === metadata.committed) await writeAtomic(metadata.path, metadata.previous);
      if (partial) await rm(partial, { recursive: true, force: true });
      if (final) await rm(final, { recursive: true, force: true });
      if (job.state !== "cancelled") { job.state = "failed"; job.error = errorText(error); job.log = log(job.log, `Render failed: ${job.error}\n`); }
    } finally { this.#children.delete(job.id); }
  }

  async #runtime(): Promise<RuntimeLayout> {
    const root = await realpath(this.#runtimeDir); const file = resolve(root, "runtime.json");
    const runtime = JSON.parse(await readFile(file, "utf8")) as RuntimeLayout;
    if (!runtime || runtime.version !== "0.8.30" || [runtime.cli, runtime.browser, runtime.ffmpeg, runtime.ffprobe].some((path) => typeof path !== "string" || isAbsolute(path) || path.includes(".."))) throw new Error("Invalid staged HyperFrames runtime manifest.");
    return runtime;
  }
  #canonical(projectDir: string): string {
    const project = realpathSync(resolve(projectDir));
    if (!statSyncNow(project).isDirectory()) throw new Error("Selected HyperFrames project is not a directory.");
    return project;
  }
  async #project(projectDir: string): Promise<string> { return this.#canonical(projectDir); }
  async #directory(project: string, path: string, create: boolean): Promise<string> {
    if (isAbsolute(path) || path.split(/[\\/]/).some((part) => !part || part === "." || part === "..")) throw new Error("Unsafe HyperFrames project path.");
    let current = project;
    for (const part of path.split(/[\\/]/)) {
      current = join(current, part);
      if (await present(current)) {
        const info = await lstat(current); if (info.isSymbolicLink() || !(await stat(current)).isDirectory()) throw new Error("HyperFrames refuses unsafe project paths.");
        if (!inside(project, await realpath(current))) throw new Error("HyperFrames path escapes the selected project.");
      } else if (create) await mkdir(current); else continue;
    }
    return resolve(project, path);
  }
  async #composition(project: string, id: string): Promise<HyperframesComposition> {
    if (!ID.test(id)) throw new Error("Invalid HyperFrames composition ID.");
    const directory = await this.#directory(project, join("hyperframes", id), false); const metadata = join(directory, "composition.json"); const source = join(directory, "index.html");
    if (!(await present(metadata)) || !(await present(source)) || (await lstat(metadata)).isSymbolicLink() || (await lstat(source)).isSymbolicLink()) throw new Error(`HyperFrames composition not found: ${id}`);
    const stored = JSON.parse(await readFile(metadata, "utf8")) as StoredComposition; if (stored.schema !== 1 || stored.id !== id || typeof stored.name !== "string") throw new Error(`Invalid HyperFrames composition metadata: ${id}`);
    const html = await readFile(source, "utf8"); const settings = this.#settings(stored.settings); const current = revision(html, settings);
    const fingerprint = stored.inputFingerprint ? await this.#sourceFingerprint(directory) : undefined;
    return { ...stored, html, settings, revision: current, rendered: stored.rendered?.revision === current && stored.inputFingerprint === fingerprint ? stored.rendered : undefined };
  }
  #settings(value: HyperframesSettings): HyperframesSettings {
    const settings = { ...DEFAULT_HYPERFRAMES_SETTINGS, ...value };
    if (!["title", "lower-third", "stat", "custom"].includes(settings.template) || ![settings.duration, settings.width, settings.height, settings.fps].every(Number.isFinite) || settings.duration <= 0 || settings.width < 16 || settings.height < 16 || settings.fps < 1 || settings.fps > 240 || ![settings.title, settings.subtitle, settings.accent, settings.background].every((item) => typeof item === "string")) throw new Error("Invalid HyperFrames settings.");
    return settings;
  }
  async #sourceFingerprint(source: string): Promise<string> {
    const hash = createHash("sha256");
    const visit = async (directory: string, prefix: string): Promise<void> => {
      for (const entry of (await readdir(directory, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
        if (entry.name === ".revisions" || entry.name === "composition.json") continue;
        const file = join(directory, entry.name); const key = join(prefix, entry.name);
        if (entry.isSymbolicLink()) throw new Error("HyperFrames refuses symlinked composition assets.");
        if (entry.isDirectory()) { hash.update(`D:${key}\0`); await visit(file, key); }
        else if (entry.isFile()) { hash.update(`F:${key}\0`); hash.update(await readFile(file)); }
      }
    };
    await visit(source, ""); return hash.digest("hex").slice(0, 24);
  }
  async #snapshot(project: string, composition: HyperframesComposition): Promise<Snapshot> {
    const source = await this.#directory(project, join("hyperframes", composition.id), false); const fingerprint = await this.#sourceFingerprint(source);
    const snapshot = await this.#directory(project, join("hyperframes", composition.id, ".revisions", composition.revision, randomUUID()), true);
    await this.#copyTree(source, snapshot); return { dir: snapshot, fingerprint };
  }
  async #copyTree(source: string, target: string): Promise<void> {
    for (const entry of await readdir(source, { withFileTypes: true })) {
      if (entry.name === ".revisions") continue;
      const from = join(source, entry.name); const to = join(target, entry.name);
      if (entry.isSymbolicLink()) throw new Error("HyperFrames refuses symlinked composition assets.");
      if (entry.isDirectory()) { if (!(await present(to))) await mkdir(to); else if ((await lstat(to)).isSymbolicLink() || !(await stat(to)).isDirectory()) throw new Error("Unsafe snapshot path."); await this.#copyTree(from, to); }
      else if (entry.isFile()) { if (!(await present(to))) await copyFile(from, to, constants.COPYFILE_EXCL); else if ((await lstat(to)).isSymbolicLink()) throw new Error("Unsafe snapshot file."); }
    }
  }
  async #probe(project: string, source: string, composition: HyperframesComposition): Promise<HyperframesRenderResult> {
    const runtime = await this.#runtime(); const ffprobe = resolve(this.#runtimeDir, runtime.ffprobe);
    if (composition.settings.transparent) {
      const frames = (await readdir(source)).filter((name) => /\.png$/i.test(name)).sort(); if (!frames.length) throw new Error("Transparent render produced no PNG frames.");
      const data = JSON.parse(await this.#exec(ffprobe, ["-v", "error", "-show_entries", "stream=width,height,pix_fmt", "-of", "json", join(source, frames[0])], dirname(source))) as { streams?: Array<{ width?: number; height?: number; pix_fmt?: string }> };
      const stream = data.streams?.[0]; if (!stream?.width || !stream.height || stream.pix_fmt !== "rgba") throw new Error("Transparent render did not produce RGBA PNG frames.");
      return { compositionId: composition.id, source: relativeProject(project, source), format: "png-sequence", width: stream.width, height: stream.height, fps: composition.settings.fps, duration: frames.length / composition.settings.fps, revision: composition.revision };
    }
    const data = JSON.parse(await this.#exec(ffprobe, ["-v", "error", "-show_entries", "stream=width,height,r_frame_rate:format=duration", "-of", "json", source], dirname(source))) as { streams?: Array<{ width?: number; height?: number; r_frame_rate?: string }>; format?: { duration?: string } };
    const stream = data.streams?.[0]; const [n, d] = stream?.r_frame_rate?.split("/").map(Number) ?? []; const duration = Number(data.format?.duration);
    if (!stream?.width || !stream.height || !Number.isFinite(duration) || duration <= 0) throw new Error("Could not verify rendered MP4 output.");
    return { compositionId: composition.id, source: relativeProject(project, source), format: "mp4", width: stream.width, height: stream.height, fps: n && d ? n / d : composition.settings.fps, duration, revision: composition.revision };
  }
  async #exec(command: string, args: string[], cwd: string): Promise<string> {
    const child = spawn(command, args, { cwd, shell: false, windowsHide: true, env: this.#environment(cwd), stdio: ["ignore", "pipe", "pipe"] }); let output = "";
    child.stdout?.on("data", (chunk: Buffer) => output = log(output, chunk.toString())); child.stderr?.on("data", (chunk: Buffer) => output = log(output, chunk.toString()));
    const code = await this.#wait(child); if (code !== 0) throw new Error(`ffprobe failed: ${output.trim() || `exit ${code}`}`); return output;
  }
  #wait(child: ChildProcess): Promise<number | null> {
    if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve(child.exitCode);
    const completion = Promise.withResolvers<number | null>();
    child.once("error", completion.reject);
    child.once("close", completion.resolve);
    return completion.promise;
  }
  async #spawn(project: string, cwd: string, args: string[], output?: (chunk: string) => void): Promise<ChildProcess> {
    const runtime = await this.#runtime();
    const privateRoot = await this.#directory(project, join(".hyperframes-engine", "runtime", randomUUID()), true);
    await Promise.all([
      mkdir(join(privateRoot, "home", "AppData", "Roaming"), { recursive: true }),
      mkdir(join(privateRoot, "home", "AppData", "Local"), { recursive: true }),
      mkdir(join(privateRoot, "tmp")),
    ]);
    const child = spawn(this.#nodePath, [resolve(this.#runtimeDir, runtime.cli), ...args], { cwd, shell: false, windowsHide: true, detached: process.platform !== "win32", env: this.#environment(privateRoot, runtime), stdio: ["ignore", "pipe", "pipe"] });
    child.stdout?.on("data", (chunk: Buffer) => output?.(chunk.toString())); child.stderr?.on("data", (chunk: Buffer) => output?.(chunk.toString())); child.on("error", (error) => output?.(`Child process error: ${errorText(error)}\n`)); return child;
  }
  #environment(privateRoot: string, runtime?: RuntimeLayout): NodeJS.ProcessEnv {
    const path = process.platform === "win32" ? `${dirname(this.#nodePath)}${delimiter}${process.env.SystemRoot ? join(process.env.SystemRoot, "System32") : ""}` : "/usr/bin:/bin";
    const file = (key: keyof Omit<RuntimeLayout, "version">) => runtime ? resolve(this.#runtimeDir, runtime[key]) : "";
    const home = join(privateRoot, "home");
    const temporary = join(privateRoot, "tmp");
    return {
      PATH: path, HOME: home, USERPROFILE: home,
      APPDATA: join(home, "AppData", "Roaming"), LOCALAPPDATA: join(home, "AppData", "Local"),
      TMP: temporary, TEMP: temporary, TMPDIR: temporary,
      SystemRoot: process.env.SystemRoot, COMSPEC: process.env.COMSPEC, PATHEXT: process.env.PATHEXT,
      ELECTRON_RUN_AS_NODE: "1",
      HYPERFRAMES_BROWSER_PATH: file("browser"), PRODUCER_HEADLESS_SHELL_PATH: file("browser"),
      HYPERFRAMES_FFMPEG_PATH: file("ffmpeg"), HYPERFRAMES_FFPROBE_PATH: file("ffprobe"),
      HYPERFRAMES_PREVIEW_HOST: "127.0.0.1", HYPERFRAMES_NO_TELEMETRY: "1", DO_NOT_TRACK: "1", NO_COLOR: "1",
    };
  }
  async #kill(child: ChildProcess): Promise<void> {
    if (child.exitCode !== null || child.signalCode !== null || !child.pid) return;
    const exited = this.#wait(child);
    if (process.platform === "win32") {
      const killer = spawn("taskkill", ["/pid", String(child.pid), "/T", "/F"], { shell: false, windowsHide: true, stdio: "ignore" });
      await this.#wait(killer);
      await exited;
      return;
    }
    try { process.kill(-child.pid, "SIGTERM"); } catch { child.kill("SIGTERM"); }
    let timer: NodeJS.Timeout | undefined;
    try {
      const stopped = await Promise.race([
        exited.then(() => true),
        new Promise<boolean>((done) => { timer = setTimeout(() => done(false), 5_000); }),
      ]);
      if (!stopped) {
        try { process.kill(-child.pid, "SIGKILL"); } catch { child.kill("SIGKILL"); }
        await exited;
      }
    } finally {
      clearTimeout(timer);
    }
  }
  #running(job: HyperframesJob): void { if (this.#disposed || job.state !== "running") throw new Error("HyperFrames render was cancelled."); }
  #copyJob(job: HyperframesJob): HyperframesJob { return { ...job, result: job.result ? { ...job.result } : undefined }; }
  #live(): void { if (this.#disposed) throw new Error("HyperFrames engine has been disposed."); }
}
