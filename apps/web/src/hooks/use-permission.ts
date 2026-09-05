/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { onCleanup, onMount, createSignal } from "solid-js";

export type PermissionState = "granted" | "denied" | "prompt" | "unsupported";

export function usePermissionState(name: string) {
  const [state, setState] = createSignal<PermissionState>("prompt");
  let status: PermissionStatus | null = null;
  const handleChange = () => {
    if (status) setState(status.state as PermissionState);
  };

  onMount(async () => {
    if (!navigator.permissions?.query) {
      setState("unsupported");
      return;
    }
    try {
      status = await navigator.permissions.query({ name: name as PermissionName });
      setState(status.state as PermissionState);
      status.addEventListener("change", handleChange);
    } catch {
      setState("unsupported");
    }
  });

  onCleanup(() => {
    status?.removeEventListener("change", handleChange);
  });

  return state;
}
