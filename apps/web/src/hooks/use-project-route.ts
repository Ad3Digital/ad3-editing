/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { useParams } from "@solidjs/router";
import { createMemo } from 'solid-js';

/**
 * Route for the editor with the project `id` loaded (see `/projects/*ref` in
 * app.tsx). The id, not the folder name: a project keeps it when it is
 * renamed, so the link keeps working (and so does the restored last route).
 */
export const projectRoute = (id: string): string => `/projects/${encodeURIComponent(id)}`;

/**
 * The `/projects/*ref` segment: a project id, or a folder name for links made
 * before ids existed. `resolveProject` in @/projects decides which it is; the
 * editor then rewrites the URL to the id it finds.
 */
export function useProjectRef() {
  const params = useParams<{ ref?: string }>();
  return createMemo(() => params.ref ?? '');
}
