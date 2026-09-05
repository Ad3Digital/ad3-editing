/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { spawnSync } from "node:child_process";
import { platform } from "node:os";

export type FontVariant = {
  weight: string;
  style: "normal" | "italic";
  source: string;
};

export type FontFamily = {
  family: string;
  variants: FontVariant[];
};

// JXA script that walks every registered macOS font family via NSFontManager
// and emits each variant's CSS-style weight, italic flag, and local() source.
// Embedded inline so the CLI binary remains self-contained.
const LIST_FONTS_JXA = `
ObjC.import("AppKit");

function nsfmWeightToCss(w) {
  if (w <= 1) return "100";
  if (w <= 2) return "200";
  if (w <= 3) return "300";
  if (w <= 5) return "400";
  if (w <= 6) return "500";
  if (w <= 8) return "600";
  if (w <= 9) return "700";
  if (w <= 11) return "800";
  return "900";
}

function run() {
  var fm = $.NSFontManager.sharedFontManager;
  var families = fm.availableFontFamilies;
  var out = [];
  for (var i = 0; i < families.count; i++) {
    var family = ObjC.unwrap(families.objectAtIndex(i));
    if (family.charAt(0) === ".") continue;
    var members = fm.availableMembersOfFontFamily(family);
    if (!members || members.isNil()) continue;
    var variants = [];
    for (var j = 0; j < members.count; j++) {
      var m = members.objectAtIndex(j);
      var fontName = ObjC.unwrap(m.objectAtIndex(0));
      var styleName = ObjC.unwrap(m.objectAtIndex(1));
      var weight = ObjC.unwrap(m.objectAtIndex(2));
      var traits = ObjC.unwrap(m.objectAtIndex(3));
      var fullName = styleName === "Regular" ? family : family + " " + styleName;
      variants.push({
        weight: nsfmWeightToCss(weight),
        style: (traits & 1) !== 0 ? "italic" : "normal",
        source: "local('" + fullName + "'), local('" + fontName + "')",
      });
    }
    if (variants.length > 0) out.push({ family: family, variants: variants });
  }
  return JSON.stringify(out);
}
`;

// Windows PowerShell exposes installed font families through System.Drawing.
// FontStyle reports the four concrete variants Windows can guarantee without
// parsing font files or relying on optional third-party utilities.
const LIST_FONTS_POWERSHELL = `
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Add-Type -AssemblyName System.Drawing
$collection = New-Object System.Drawing.Text.InstalledFontCollection
$styles = @(
  @{ Value = [System.Drawing.FontStyle]::Regular; Weight = "400"; Style = "normal" },
  @{ Value = [System.Drawing.FontStyle]::Bold; Weight = "700"; Style = "normal" },
  @{ Value = [System.Drawing.FontStyle]::Italic; Weight = "400"; Style = "italic" },
  @{ Value = ([System.Drawing.FontStyle]::Bold -bor [System.Drawing.FontStyle]::Italic); Weight = "700"; Style = "italic" }
)
$out = foreach ($family in $collection.Families) {
  $variants = @()
  foreach ($entry in $styles) {
    if ($family.IsStyleAvailable($entry.Value)) {
      $variants += [pscustomobject]@{
        weight = $entry.Weight
        style = $entry.Style
        source = 'local("' + $family.Name + '")'
      }
    }
  }
  if ($variants.Count -gt 0) {
    [pscustomobject]@{ family = $family.Name; variants = $variants }
  }
}
$sorted = @($out | Sort-Object family)
ConvertTo-Json -InputObject $sorted -Compress -Depth 4
`;

export type ListLocalFontsOptions = {
  familyPattern?: string;
  weights?: string[];
  style?: "normal" | "italic";
  limit?: number;
};

export function listLocalFonts(options: ListLocalFontsOptions = {}): FontFamily[] {
  const os = platform();
  const result =
    os === "darwin"
      ? spawnSync("osascript", ["-l", "JavaScript", "-e", LIST_FONTS_JXA], {
          encoding: "utf8",
          maxBuffer: 16 * 1024 * 1024,
        })
      : os === "win32"
        ? spawnSync(
            "powershell.exe",
            ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", LIST_FONTS_POWERSHELL],
            { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
          )
        : null;
  if (!result) throw new Error("fonts is supported on macOS and Windows.");
  if (result.error) throw new Error(result.error.message);
  if (result.status !== 0) {
    throw new Error(result.stderr.trim() || "Failed to enumerate fonts.");
  }

  const all = JSON.parse(result.stdout.trim() || "[]") as FontFamily[];
  const pattern = options.familyPattern?.toLowerCase();
  const weights = options.weights && options.weights.length > 0 ? new Set(options.weights) : null;
  const { style, limit } = options;

  const out: FontFamily[] = [];
  for (const family of all) {
    if (pattern && !family.family.toLowerCase().includes(pattern)) continue;
    const variants = family.variants.filter((v) => {
      if (weights && !weights.has(v.weight)) return false;
      if (style && v.style !== style) return false;
      return true;
    });
    if (variants.length === 0) continue;
    out.push({ family: family.family, variants });
    if (limit !== undefined && out.length >= limit) break;
  }
  return out;
}
