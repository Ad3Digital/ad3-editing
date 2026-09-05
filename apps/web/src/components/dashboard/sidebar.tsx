/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { Show, type JSX } from "solid-js";
import { Icon } from "@/components/ui/icon";
import { useAuth } from "@/context/auth";
import { useAvatar } from "@/hooks/use-avatar";
import { cx } from "@/lib/cva";

type DashboardSidebarItemProps = {
  icon: string;
  label: string;
  active?: boolean;
  onClick?(): void;
  class?: string;
};

export function DashboardSidebarItem(props: DashboardSidebarItemProps) {
  return (
    <button
      type="button"
      onClick={props.onClick}
      class={cx(
        "my-0.5 flex h-7 w-full shrink-0 items-center gap-1 rounded-md pl-0 pr-1 hover:bg-accent focus-ring",
        props.class,
      )}
      classList={{ "bg-accent": props.active }}
    >
      <span class="grid size-7 shrink-0 place-items-center overflow-clip">
        <Show
          when={props.active}
          fallback={<Icon name={props.icon} class="size-6 text-muted-foreground" />}
        >
          <Icon name={props.icon} class="size-6 text-foreground" />
        </Show>
      </span>
      <span
        class="min-w-0 flex-1 truncate text-left text-xs text-muted-foreground"
        classList={{ "text-foreground": props.active }}
      >
        {props.label}
      </span>
    </button>
  );
}

export function DashboardSidebarHeader() {
  return (
    // Extra top padding on the macOS desktop build clears the traffic lights
    // (hiddenInset title bar), except in fullscreen where they are gone.
    <div class="flex flex-col items-start gap-3 p-4 [[data-platform=darwin]:not([data-fullscreen=true])_&]:pt-14">
      <Icon name="ad3-editing-large" class="size-6 text-muted-foreground" />
      <div class="flex w-full flex-col items-start gap-1 text-muted-foreground">
        <p class="w-full text-2xl leading-5 font-450 text-muted-foreground">
          AD3 Editing
        </p>
        <div class="flex w-full items-center py-0.5">
          <p class="w-full overflow-hidden text-xxs whitespace-nowrap text-ellipsis text-muted-foreground opacity-50">
            v{APP_VERSION}
          </p>
        </div>
      </div>
    </div>
  );
}

type DashboardSidebarNavProps = {
  children: JSX.Element;
  footer: JSX.Element;
};

export function DashboardSidebarNav(props: DashboardSidebarNavProps) {
  return (
    <div class="flex min-h-0 flex-1 flex-col gap-2 px-3">
      <div class="flex shrink-0 flex-col">
        <div class="flex h-8 shrink-0 items-center px-1">
          <p class="text-xs text-muted-foreground">
            Get Started
          </p>
        </div>
        {props.children}
      </div>
      <div class="min-h-0 flex-1" />
      <div class="flex shrink-0 flex-col">
        {props.footer}
      </div>
    </div>
  );
}

type DashboardSidebarUserProps = {
  active: boolean;
  onClick: () => void;
};

export function DashboardSidebarLocal() {
  return (
    <div class="p-2">
      <div class="flex w-full items-center gap-2 rounded-md p-2 text-left">
        <div class="grid size-8 shrink-0 place-items-center rounded-full bg-input text-xs text-foreground">
          L
        </div>
        <div class="flex min-w-0 flex-1 flex-col justify-center">
          <span class="truncate text-xs font-450 text-foreground">Local workspace</span>
          <span class="truncate text-xxs text-muted-foreground">No account required</span>
        </div>
      </div>
    </div>
  );
}

export function DashboardSidebarUser(props: DashboardSidebarUserProps) {
  const auth = useAuth();

  const displayName = () => {
    const user = auth.user();
    return user?.user_metadata?.full_name || user?.email || "User";
  };

  const planLabel = () => (auth.isPro() ? "Pro Plan" : "Free Plan");

  const initial = () => displayName().charAt(0).toUpperCase();
  const avatarUrl = useAvatar();

  return (
    <div class="p-2">
      <button
        type="button"
        onClick={props.onClick}
        class="flex w-full items-center gap-2 rounded-md p-2 text-left hover:bg-accent focus-ring"
        classList={{ "bg-accent": props.active }}
      >
        <Show
          when={avatarUrl()}
          fallback={
            <div class="grid size-8 shrink-0 place-items-center rounded-full bg-input text-xs text-foreground">
              {initial()}
            </div>
          }
        >
          {(url) => (
            <img
              src={url()}
              alt=""
              class="size-8 shrink-0 rounded-full object-cover"
            />
          )}
        </Show>
        <div class="flex min-w-0 flex-1 flex-col justify-center">
          <span class="truncate text-xs font-450 text-foreground">
            {displayName()}
          </span>
          <span class="truncate text-xxs text-muted-foreground">
            {planLabel()}
          </span>
        </div>
      </button>
    </div>
  );
}
