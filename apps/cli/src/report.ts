/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { spawn } from "node:child_process";
import { arch, platform, release } from "node:os";

const REPO = "Ad3Digital/ad3-editing";

export const GH_MISSING =
  "gh (GitHub CLI) is not installed, so the issue cannot be filed. Install it from https://cli.github.com, run `gh auth login`, then retry.";

export type IssueInput = {
  title: string;
  body?: string;
  commands?: string[];
  logs?: string[];       // already formatted log lines, oldest first
  appStatus: string;     // "running", "not running", "not checked", or why it was unreachable
  version: string;
};

function fence(language: string, content: string): string {
  return `\`\`\`${language}\n${content}\n\`\`\``;
}

function environmentTable(input: IssueInput): string {
  const rows: Array<[string, string]> = [
    ["dapi", input.version],
    ["platform", `${platform()} ${release()} (${arch()})`],
    ["node", process.version],
    ["app", input.appStatus],
  ];
  return ["| | |", "| --- | --- |", ...rows.map(([k, v]) => `| ${k} | ${v} |`)].join("\n");
}

export function buildIssueBody(input: IssueInput): string {
  const sections: string[] = [];

  if (input.body?.trim()) sections.push(input.body.trim());
  if (input.commands?.length) sections.push(`## Repro\n\n${fence("sh", input.commands.join("\n"))}`);
  sections.push(`## Environment\n\n${environmentTable(input)}`);
  if (input.logs?.length) sections.push(`## App logs\n\n${fence("", input.logs.join("\n"))}`);

  return `${sections.join("\n\n")}\n`;
}

// --repo is explicit because dapi runs from the user's project, not a checkout
// of the editor; the body goes over stdin so a long log tail can't blow the
// argv size limit.
export function createIssue(title: string, body: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const gh = spawn("gh", ["issue", "create", "--repo", REPO, "--title", title, "--body-file", "-"]);

    let stdout = "";
    let stderr = "";
    gh.stdout.on("data", (chunk) => (stdout += chunk));
    gh.stderr.on("data", (chunk) => (stderr += chunk));

    gh.on("error", (e) => {
      reject((e as NodeJS.ErrnoException).code === "ENOENT" ? new Error(GH_MISSING) : e);
    });
    gh.on("close", (code) => {
      if (code !== 0) {
        // gh explains itself well (not authenticated, no access, rate limited).
        reject(new Error(stderr.trim() || `gh issue create exited with code ${code}`));
        return;
      }
      const url = stdout.trim().split("\n").pop() ?? "";
      if (!url.startsWith("https://")) {
        reject(new Error(`gh issue create did not print an issue URL: ${stdout.trim() || "(no output)"}`));
        return;
      }
      resolve(url);
    });

    gh.stdin.on("error", () => { }); // gh exiting early (auth failure) breaks the pipe
    gh.stdin.end(body);
  });
}
