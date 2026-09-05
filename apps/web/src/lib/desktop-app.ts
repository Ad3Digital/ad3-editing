/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { toast } from "somoto";

import { track } from "@/lib/analytics";

/** The installer name is stable so GitHub can resolve the latest Windows build. */
const DOWNLOAD_URL =
  "https://github.com/Ad3Digital/ad3-editing/releases/latest/download/AD3-Editing-x64-Setup.exe";

/** Where a download was started from, so the promos can be compared. */
export type DesktopAppDownloadSource = "canvas_banner" | "dashboard_footer" | "main_menu";

function isWindows() {
  const uaData = (navigator as { userAgentData?: { platform?: string } }).userAgentData;
  return /^win/i.test(uaData?.platform || navigator.platform);
}

/** Downloads the Windows x64 installer; other platforms get an explicit notice. */
export function downloadDesktopApp(source: DesktopAppDownloadSource) {
  const supported = isWindows();
  track("desktop_app_download", { source, supported });

  if (!supported) {
    toast("Desktop download unavailable in this browser", {
      description:
        "AD3 Editing currently publishes Windows x64 builds. Source code is available on GitHub for other platforms.",
    });
    return;
  }

  const a = document.createElement("a");
  a.href = DOWNLOAD_URL;
  a.download = "";
  document.body.appendChild(a);
  a.click();
  a.remove();
}
