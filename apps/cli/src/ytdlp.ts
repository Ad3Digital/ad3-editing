/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { spawn, spawnSync } from "node:child_process";

// Resolve the binary once. YT_DLP_PATH mirrors DIFFUSION_APP_PATH: an escape
// hatch for pinned or non-PATH installs.
const BIN = process.env.YT_DLP_PATH ?? "yt-dlp";

// Fails with an actionable message before any download is attempted. ENOENT is
// the "not installed" case; a non-zero status means the binary is present but
// broken.
function assertInstalled(): void {
  const probe = spawnSync(BIN, ["--version"], { encoding: "utf8" });
  if (probe.error) {
    if ((probe.error as NodeJS.ErrnoException).code === "ENOENT") {
      const install =
        process.platform === "win32"
          ? "winget install yt-dlp.yt-dlp, or pipx install yt-dlp"
          : "brew install yt-dlp, or pipx install yt-dlp";
      throw new Error(`yt-dlp not found. Install it (${install}) or set YT_DLP_PATH to its location.`);
    }
    throw new Error(probe.error.message);
  }
  if (probe.status !== 0) {
    throw new Error(probe.stderr?.trim() || "yt-dlp is present but not runnable.");
  }
}

export type FetchOptions = {
  output?: string;
  format?: string;
  audio?: boolean;
  raw?: string[];
};

// Downloads the URL and resolves to the final file path(s) on disk. yt-dlp's
// own `after_move:filepath` reports what actually landed (post extraction /
// rename), so we never guess the name. --quiet silences stdout chatter while
// --progress forces the progress bar to stderr, keeping stdout to just the
// paths we capture.
export function fetchVideo(url: string, opts: FetchOptions = {}): Promise<string[]> {
  assertInstalled();

  const args = ["--quiet", "--no-warnings", "--progress", "--print", "after_move:filepath"];
  if (opts.output) args.push("-o", opts.output);
  if (opts.format) {
    args.push("-f", opts.format);
  } else if (!opts.audio) {
    // Default to mp4: prefer mp4/m4a streams, then remux the merged result so
    // the landed file is a .mp4 even when only WebM/mkv sources were available.
    // An explicit -f or --audio opts out.
    args.push("-f", "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b", "--merge-output-format", "mp4");
  }
  if (opts.audio) args.push("-x");
  if (opts.raw?.length) args.push(...opts.raw);
  args.push(url);

  const { promise, resolve, reject } = Promise.withResolvers<string[]>();
  // stdout: resolved paths (captured); stderr: live progress (inherited).
  const child = spawn(BIN, args, { stdio: ["ignore", "pipe", "inherit"] });
  let out = "";
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk) => (out += chunk));
  child.on("error", reject);
  child.on("close", (code) => {
    if (code === 0) {
      resolve(out.split("\n").map((line) => line.trim()).filter(Boolean));
    } else {
      reject(new Error(`yt-dlp exited with code ${code}.`));
    }
  });
  return promise;
}
