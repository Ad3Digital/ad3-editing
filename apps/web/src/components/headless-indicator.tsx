/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { Show } from "solid-js";
import { useAuth } from "@/context/auth";

export function HeadlessIndicator() {
  const auth = useAuth();

  return (
    <Show when={auth.headless()}>
      <div
        class="pointer-events-none fixed top-3 left-1/2 z-[9999] flex -translate-x-1/2 items-center gap-2 rounded-full border border-border bg-background/90 px-3 py-1.5 shadow-md backdrop-blur"
      >
        <span class="relative flex size-2">
          <span class="absolute inline-flex size-full animate-ping rounded-full bg-primary opacity-75" />
          <span class="relative inline-flex size-2 rounded-full bg-primary" />
        </span>
        <span class="text-xs font-450 text-foreground">Remote controlled</span>
      </div>
    </Show>
  );
}
