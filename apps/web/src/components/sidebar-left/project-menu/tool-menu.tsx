/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import {
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuGroup,
} from "@/components/ui/dropdown-menu";
import { Show } from "solid-js";
import { usePromptInput } from "@/context/prompt-input";
import { useWorld } from "@diffusionstudio/koota-solid";
import { Tool, ToolType } from "@diffusionstudio/runtime";
import { isLocalOnlyDesktop } from "@/lib/local-mode";
import { splitAtPlayhead, trimLeftAtPlayhead, trimRightAtPlayhead } from "@/engine/split";

export function ToolMenu() {
  const { setPromptInputOpen } = usePromptInput();
  const world = useWorld();
  const localOnly = isLocalOnlyDesktop();
  const setTool = (value: ToolType) => world.set(Tool, { value });

  return (
    <>
      <Show when={!localOnly}>
        <DropdownMenuGroup>
          <DropdownMenuItem onSelect={() => setPromptInputOpen(true)}>
            Generate with AI...
          </DropdownMenuItem>
        </DropdownMenuGroup>

        <DropdownMenuSeparator />
      </Show>

      <DropdownMenuGroup>
        <DropdownMenuItem onSelect={() => splitAtPlayhead(world)}>
          Cut at playhead
          <DropdownMenuShortcut>W</DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => trimLeftAtPlayhead(world)}>
          Ripple delete to previous cut
          <DropdownMenuShortcut>Q</DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => trimRightAtPlayhead(world)}>
          Ripple delete to next cut
          <DropdownMenuShortcut>E</DropdownMenuShortcut>
        </DropdownMenuItem>
      </DropdownMenuGroup>

      <DropdownMenuSeparator />

      <DropdownMenuGroup>
        <DropdownMenuItem onSelect={() => setTool(ToolType.SCENE)}>
          Scene
          <DropdownMenuShortcut>F</DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => setTool(ToolType.TEXT)}>
          Text
          <DropdownMenuShortcut>T</DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => setTool(ToolType.RECT)}>
          Rectangle
          <DropdownMenuShortcut>R</DropdownMenuShortcut>
        </DropdownMenuItem>
      </DropdownMenuGroup>
    </>
  );
}
