/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { toast } from "somoto";
import {
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuGroup,
} from "@/components/ui/dropdown-menu";
import { useAuth } from "@/context/auth";

export function HelpMenu() {
  const auth = useAuth();

  const copyUserId = async () => {
    const id = auth.user()?.id;
    if (!id) return;

    try {
      await navigator.clipboard.writeText(id);
      toast("Copied!", { description: "The user id has been copied to your clipboard." });
    } catch (error) {
      toast("Failed to copy", { description: error instanceof Error ? error.message : "Unknown error" });
    }
  };

  return (
    <>
      <DropdownMenuGroup>
        <DropdownMenuItem as="a" href="https://www.diffusion.studio/updates" target="_blank">
          What’s new
        </DropdownMenuItem>
        <DropdownMenuItem as="a" href="https://discord.gg/AYySWDhgNK" target="_blank">
          Discord community
        </DropdownMenuItem>
      </DropdownMenuGroup>

      <DropdownMenuSeparator />

      <DropdownMenuGroup>
        <DropdownMenuItem as="a" href="mailto:support@diffusion.studio" target="_blank">
          Report issue
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={copyUserId}>Copy user id</DropdownMenuItem>
        <DropdownMenuItem as="a" href="https://discord.gg/AYySWDhgNK" target="_blank">
          Contact support
        </DropdownMenuItem>
      </DropdownMenuGroup>
    </>
  );
}
