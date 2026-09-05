/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogPortal } from "@/components/ui/dialog";
import { Icon } from "@/components/ui/icon";
import { useAuth } from "@/context/auth";
import { cx } from "@/lib/cva";
import { trpc } from "@/lib/trpc";
import { For, Show, children, createResource, type JSX } from "solid-js";
import { Separator } from "../ui/separator";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";


type DashboardLabelValueProps = {
  label: JSX.Element;
  value: JSX.Element;
  labelClass?: string;
  valueClass?: string;
};

export function DashboardLabelValue(props: DashboardLabelValueProps) {
  return (
    <div class="flex min-w-0 flex-1 flex-col gap-1">
      <p class={cx("text-foreground text-xs", props.labelClass)}>
        {props.label}
      </p>
      <p class={cx("text-muted-foreground text-xs", props.valueClass)}>
        {props.value}
      </p>
    </div>
  );
}

type DashboardSurfaceCardProps = {
  class?: string;
  children: JSX.Element;
};

export function DashboardSurfaceCard(props: DashboardSurfaceCardProps) {
  return (
    <div class={cx("rounded-xl bg-accent/50 p-4", props.class)}>
      {props.children}
    </div>
  );
}

type DashboardTitledSectionProps = {
  title: string;
  class?: string;
  children: JSX.Element;
};


export function DashboardTitledSection(props: DashboardTitledSectionProps) {
  return (
    <section class={cx("flex flex-col", props.class)}>
      <div class={cx("flex h-9 items-center px-2 text-sm leading-5 font-450 text-foreground", props.class)}>
        {props.title}
      </div>
      {props.children}
    </section>
  );
}

type DashboardSurfaceSectionProps = {
  title: string;
  class?: string;
  children: JSX.Element;
};

export function DashboardSurfaceSection(props: DashboardSurfaceSectionProps) {
  return (
    <DashboardTitledSection title={props.title} class={props.class}>
      <DashboardSurfaceCard class="flex flex-col gap-3">
        {props.children}
      </DashboardSurfaceCard>
    </DashboardTitledSection>
  );
}

type DashboardScrollViewProps = {
  class?: string;
  children: JSX.Element;
};


export function DashboardScrollView(props: DashboardScrollViewProps) {
  return (
    <div class={cx("min-h-0 flex-1 overflow-y-auto px-6 py-12", props.class)}>
      <div class="mx-auto flex w-full max-w-3xl flex-col gap-6">{props.children}</div>
    </div>
  );
}

type DashboardFormModalProps = {
  open: boolean;
  title: string;
  onClose: () => void;
  footer: JSX.Element;
  children: JSX.Element;
};

export function DashboardFormModal(props: DashboardFormModalProps) {
  const handleOpenAutoFocus = (event: Event) => {
    event.preventDefault();
    const container = event.currentTarget as HTMLElement;

    queueMicrotask(() => {
      container.querySelector<HTMLInputElement>(
        '[data-slot="text-field-input"]:not([disabled])',
      )?.focus();
    });
  };

  const handleOpenChange = (open: boolean) => {
    if (!open) props.onClose();
  };

  return (
    <Dialog open={props.open} preventScroll={false} onOpenChange={handleOpenChange}>
      <DialogPortal>
        <DialogContent
          onOpenAutoFocus={handleOpenAutoFocus}
          showCloseButton={false}
          class="w-77 max-w-11/12 max-h-11/12 overflow-hidden rounded-xl border-border p-0 gap-0"
        >
          <div class="flex max-h-full flex-col gap-4 px-2 py-0">
            <div class="flex w-full items-center gap-2 border-b border-border pl-2 pr-0 py-2">
              <div class="min-w-0 flex-1">
                <p class="truncate text-foreground text-xs">
                  {props.title}
                </p>
              </div>
              <Tooltip>
                <TooltipTrigger
                  as={Button}
                  size="icon"
                  variant="ghost"
                  class="text-muted-foreground"
                  onClick={props.onClose}
                >
                  <Icon name="close-remove" class="text-foreground" />
                </TooltipTrigger>
                <TooltipContent>Close</TooltipContent>
              </Tooltip>
            </div>

            <div class="flex min-h-0 w-full flex-col gap-6 overflow-y-auto px-2 py-0">
              {props.children}
            </div>

            <div class="flex w-full items-center justify-end border-t border-border px-1 py-3">
              <div class="flex items-center gap-2">
                {props.footer}
              </div>
            </div>
          </div>
        </DialogContent>
      </DialogPortal>
    </Dialog>
  );
}

type DashboardPlanDetailsProps = {
  details: readonly {
    label: string;
    value: JSX.Element;
  }[];
};

export function DashboardPlanDetails(props: DashboardPlanDetailsProps) {
  return (
    <div class="flex min-w-0 flex-1 flex-col gap-4 md:flex-row md:items-center md:gap-4">
      <For each={props.details}>
        {(detail, index) => (
          <>
            <DashboardLabelValue
              label={detail.label}
              value={detail.value}
              labelClass="truncate"
              valueClass="truncate"
            />
            <Show when={index() < props.details.length - 1}>
              <div class="hidden h-8 w-px shrink-0 bg-border md:block" />
            </Show>
          </>
        )}
      </For>
    </div>
  );
}

export function DashboardFreePlanDetails() {
  return (
    <DashboardPlanDetails
      details={[
        { label: "Free", value: "$0.00" },
        { label: "AI credits", value: "50 trial credits \u00B7 One time only" },
      ]}
    />
  );
}

const RENEWAL_DATE_FORMAT = new Intl.DateTimeFormat(undefined, {
  month: "long",
  day: "numeric",
  year: "numeric",
});

function formatCurrency(amountCents: number, currency: string) {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: currency.toUpperCase(),
  }).format(amountCents / 100);
}

export function DashboardProPlanDetails() {
  const auth = useAuth();
  const [summary] = createResource(() => trpc.getSubscriptionSummary.query());

  const priceLabel = () => {
    const s = summary();
    if (!s) return "\u2014";
    const suffix = s.billingPeriod === "year" ? "/yr" : "/mo";
    return `${formatCurrency(s.amount, s.currency)}${suffix}`;
  };

  const creditsLabel = () =>
    `${auth.creditLimit().toLocaleString()} credits/mo`;

  const renewsLabel = () => {
    const s = summary();
    if (!s) return "\u2014";
    return RENEWAL_DATE_FORMAT.format(new Date(s.currentPeriodEnd));
  };

  return (
    <DashboardPlanDetails
      details={[
        { label: "Pro", value: priceLabel() },
        { label: "AI credits", value: creditsLabel() },
        { label: "Renews", value: renewsLabel() },
      ]}
    />
  );
}

type DashboardDividedStackProps = {
  class?: string;
  spacing?: "tight" | "loose";
  children: JSX.Element;
};

export function DashboardDividedStack(props: DashboardDividedStackProps) {
  const resolved = children(() => props.children);
  const items = () =>
    resolved.toArray().filter((item) => item != null && typeof item !== "boolean");
  const isLoose = () => props.spacing === "loose";

  return (
    <div
      class={cx("flex flex-col", props.class)}
      classList={{
        "gap-0": isLoose(),
        "gap-3": !isLoose(),
      }}
    >
      <For each={items()}>
        {(item, index) => (
          <>
            {item}
            <Show when={index() < items().length - 1}>
              <Show when={isLoose()} fallback={<Separator />}>
                <Separator class="my-3" />
              </Show>
            </Show>
          </>
        )}
      </For>
    </div>
  );
}

type DashboardPlanSummaryCardProps = {
  action: JSX.Element;
  children: JSX.Element;
  class?: string;
};

export function DashboardPlanSummaryCard(props: DashboardPlanSummaryCardProps) {
  return (
    <DashboardSurfaceCard class={cx("flex flex-col gap-4", props.class)}>
      <div class="flex flex-col gap-4 md:flex-row md:items-center">
        {props.children}
        {props.action}
      </div>
    </DashboardSurfaceCard>
  );
}

type DashboardFeatureRowProps = {
  label: string;
};

export function DashboardFeatureRow(props: DashboardFeatureRowProps) {
  return (
    <div class="flex items-center gap-2">
      <span class="size-4 flex items-center justify-center text-primary overflow-clip">
        <Icon name="confirm-check" />
      </span>
      <p class="text-muted-foreground text-xs">
        {props.label}
      </p>
    </div>
  );
}

type DashboardInfoActionRowProps = {
  title: string;
  titleContent?: JSX.Element | string;
  description?: JSX.Element;
  action: JSX.Element;
  leading?: JSX.Element;
  layout?: "responsive" | "responsive-md" | "inline";
};

export function DashboardInfoActionRow(props: DashboardInfoActionRowProps) {
  const layoutClass = () => {
    if (props.layout === "inline") return "flex-row items-center gap-4";
    if (props.layout === "responsive-md") return "flex-col gap-4 md:flex-row md:items-center";
    return "flex-col gap-4 sm:flex-row sm:items-center";
  };

  const hasDescription = () => props.description !== undefined;
  const leadingAlignClass = () => {
    if (hasDescription()) return "items-start";
    return "items-center";
  };

  const renderTitleText = (value: string) => (
    <p
      class={cx(
        "text-xs text-foreground",
        props.layout === "inline" ? "min-w-0 flex-1 truncate" : undefined,
      )}
    >
      {value}
    </p>
  );

  const titleContent = () => {
    const content = props.titleContent;

    if (content === undefined) return renderTitleText(props.title);
    if (typeof content === "string" || typeof content === "number") {
      return renderTitleText(String(content));
    }

    return content;
  };

  const textContent = () => (
    <div class="flex min-w-0 flex-1 flex-col gap-1">
      {titleContent()}
      <Show when={props.description}>
        <p class="text-muted-foreground text-xs">
          {props.description}
        </p>
      </Show>
    </div>
  );

  return (
    <div class={cx("flex", layoutClass())}>
      <Show
        when={props.leading}
        fallback={
          textContent()
        }
      >
        <div class={cx("flex min-w-0 flex-1 gap-2", leadingAlignClass())}>
          <span class="grid size-6 shrink-0 place-items-center text-muted-foreground">
            {props.leading}
          </span>
          {textContent()}
        </div>
      </Show>

      {props.action}
    </div>
  );
}

type DashboardCardButtonProps = {
  active?: boolean;
  onClick?(): void;
  onDoubleClick?(): void;
  onEscape?(): void;
  onDelete?(): void;
  children: JSX.Element;
  class?: string;
};

export function DashboardCardButton(props: DashboardCardButtonProps) {
  const handleKeyDown: JSX.EventHandlerUnion<HTMLDivElement, KeyboardEvent> = (event) => {
    if (event.target !== event.currentTarget) return;
    if (event.key === "Enter") {
      const activate = props.onDoubleClick ?? props.onClick;
      if (!activate) return;

      event.preventDefault();
      activate();
      return;
    }

    if (event.key === " ") {
      if (!props.onClick) return;

      event.preventDefault();
      props.onClick();
      return;
    }

    if (event.key === "Escape") {
      if (!props.onEscape) return;

      event.preventDefault();
      props.onEscape();
      return;
    }

    if (event.key === "Backspace" || event.key === "Delete") {
      if (!props.onDelete) return;

      event.preventDefault();
      props.onDelete();
    }
  };

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={props.onClick}
      onDblClick={props.onDoubleClick}
      onKeyDown={handleKeyDown}
      class={cx(
        "flex min-w-0 flex-col gap-3 rounded-xl px-2 pt-2 pb-3 text-left outline-none transition-colors hover:bg-accent/50 focus-ring group",
        props.active && "bg-primary/15 hover:bg-primary/15 ring-1 ring-inset ring-primary",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

type DashboardCardPreviewProps = {
  children?: JSX.Element;
  class?: string;
};

export function DashboardCardPreview(props: DashboardCardPreviewProps) {
  return (
    <div
      class={cx(
        "bg-canvas relative aspect-video w-full overflow-hidden rounded-md border border-border",
        props.class,
      )}
    >
      {props.children}
    </div>
  );
}

type DashboardCardMetaProps = {
  title: string;
  subtitle?: string;
};

export function DashboardCardMeta(props: DashboardCardMetaProps) {
  const hasSubtitle = () => props.subtitle !== undefined;

  return (
    <div class="flex flex-col gap-1 px-2">
      <p class="min-w-0 truncate text-xs   text-foreground">
        {props.title}
      </p>
      <p
        class="min-w-0 truncate text-xs   text-muted-foreground"
        classList={{ "opacity-0": !hasSubtitle() }}
        aria-hidden={hasSubtitle() ? undefined : "true"}
      >
        {props.subtitle ?? "\u00A0"}
      </p>
    </div>
  );
}

type DashboardViewSectionProps = {
  title: string;
  controls: JSX.Element;
  children: JSX.Element;
  class?: string;
  onBackgroundClick?(): void;
};

export function DashboardViewSection(props: DashboardViewSectionProps) {
  const handleClick: JSX.EventHandlerUnion<HTMLDivElement, MouseEvent> = (event) => {
    const target = event.target as HTMLElement;
    if (target !== event.currentTarget && target.dataset.slot !== "card-grid") return;

    props.onBackgroundClick?.();
  };

  return (
    <div class={cx("flex min-h-0 flex-1 flex-col gap-3 pt-4", props.class)}>
      <div class="flex items-end gap-6 px-6">
        <h1 class="min-w-0 flex-1 text-2xl leading-6 font-450 text-foreground">
          {props.title}
        </h1>
        {props.controls}
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto px-4" onClick={handleClick}>
        <div
          data-slot="card-grid"
          class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] content-start items-start gap-x-0.5 gap-y-3"
        >
          {props.children}
        </div>
      </div>
    </div>
  );
}

