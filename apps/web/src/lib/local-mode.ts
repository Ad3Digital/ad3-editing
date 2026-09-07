/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

/** Desktop builds are local editors: no account or cloud surfaces. */
export function isLocalOnlyDesktop(): boolean {
  return window.desktop?.platform === "win32" || window.desktop?.platform === "darwin";
}
