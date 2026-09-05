/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";

import { supabase } from "@/lib/supabase";
import { useAuth } from "@/context";

/**
 * Resolves a user's avatar_url (either a direct HTTP URL from OAuth
 * or a Supabase Storage path) into a displayable image URL.
 */
export function useAvatar(): Accessor<string | null> {
  const auth = useAuth();
  const [displayUrl, setDisplayUrl] = createSignal<string | null>(null);

  createEffect(() => {
    const url = auth.user()?.user_metadata?.avatar_url as string | undefined;
    if (!url) {
      setDisplayUrl(null);
      return;
    }

    if (url.startsWith("http")) {
      setDisplayUrl(url);
      return;
    }

    // Storage path — download the blob
    if (!supabase) return;
    void supabase.storage
      .from("avatars")
      .download(url)
      .then(({ data }) => {
        if (data) {
          const prev = displayUrl();
          if (prev?.startsWith("blob:")) URL.revokeObjectURL(prev);
          setDisplayUrl(URL.createObjectURL(data));
        }
      });
  });

  onCleanup(() => {
    const url = displayUrl();
    if (url?.startsWith("blob:")) URL.revokeObjectURL(url);
  });

  return displayUrl;
}
