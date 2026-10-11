/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Public wire protocol consumed by both the CLI binary and any host that
// embeds the CLI socket server (the AD3 Editing desktop app). Importing
// from `@ad3-editing/cli/protocol` is the OSS-friendly seam: this module has no
// dependency on proprietary code, so the package stays extractable into its
// own repo.
export * from "./cli-channels";
export * from "./cli-socket-path";
