/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { toast } from "somoto";

import { track } from "@/lib/analytics";

const RELEASE_URL = "https://github.com/Ad3Digital/ad3-editing/releases/latest";
const DOWNLOAD_URLS = {
  windows: "https://github.com/Ad3Digital/ad3-editing/releases/latest/download/AD3-Editing-x64-Setup.exe",
  macosArm64:
    "https://github.com/Ad3Digital/ad3-editing/releases/latest/download/AD3-Editing-darwin-arm64.dmg",
  macosX64:
    "https://github.com/Ad3Digital/ad3-editing/releases/latest/download/AD3-Editing-darwin-x64.dmg",
} as const;

type BrowserPlatform = "windows" | "macos" | "other";
type DesktopArchitecture = "arm64" | "x64" | "unknown";
type UserAgentData = {
  platform?: string;
  getHighEntropyValues?: (hints: string[]) => Promise<{ architecture?: string }>;
};

/** Where a download was started from, so the promos can be compared. */
export type DesktopAppDownloadSource = "canvas_banner" | "dashboard_footer" | "main_menu";

function getUserAgentData(): UserAgentData | undefined {
  // User-Agent Client Hints are not yet in every TypeScript DOM definition.
  const navigatorWithUserAgentData = navigator as Navigator & { userAgentData?: UserAgentData };
  return navigatorWithUserAgentData.userAgentData;
}

function browserPlatform(): BrowserPlatform {
  const platform = getUserAgentData()?.platform || navigator.platform;
  if (/^win/i.test(platform)) return "windows";
  if (/^mac/i.test(platform)) return "macos";
  return "other";
}

async function macosArchitecture(): Promise<DesktopArchitecture> {
  const userAgentData = getUserAgentData();
  if (!userAgentData?.getHighEntropyValues) return "unknown";

  try {
    const { architecture = "" } = await userAgentData.getHighEntropyValues(["architecture"]);
    if (/arm/i.test(architecture)) return "arm64";
    if (/x86|intel/i.test(architecture)) return "x64";
  } catch {
    // Browsers may decline high-entropy hints. A generic Mac UA is not enough
    // to distinguish Apple silicon from Intel, so let the user choose instead.
  }
  return "unknown";
}

/** A concise platform-specific CTA that does not guess a Mac's processor. */
export function desktopAppDownloadLabel(): string {
  switch (browserPlatform()) {
    case "windows":
      return "Download for Windows";
    case "macos":
      return "Download for macOS";
    default:
      return "Get desktop app";
  }
}

/** Downloads a native installer when its architecture is known, otherwise opens the release chooser. */
export async function downloadDesktopApp(source: DesktopAppDownloadSource) {
  const platform = browserPlatform();
  if (platform === "windows") {
    track("desktop_app_download", { source, platform, architecture: "x64", supported: true });
    window.location.assign(DOWNLOAD_URLS.windows);
    return;
  }

  if (platform === "macos") {
    const architecture = await macosArchitecture();
    track("desktop_app_download", {
      source,
      platform,
      architecture,
      supported: architecture !== "unknown",
    });
    if (architecture === "arm64") {
      window.location.assign(DOWNLOAD_URLS.macosArm64);
      return;
    }
    if (architecture === "x64") {
      window.location.assign(DOWNLOAD_URLS.macosX64);
      return;
    }

    window.location.assign(RELEASE_URL);
    toast("Choose your macOS installer", {
      description:
        "Your browser does not disclose the Mac processor. On the release page, choose Apple silicon (arm64) or Intel (x64).",
    });
    return;
  }

  track("desktop_app_download", { source, platform, architecture: "unknown", supported: false });
  toast("Desktop download unavailable in this browser", {
    description: "AD3 Editing publishes Windows and macOS desktop builds. Visit GitHub Releases from a supported computer.",
  });
}
