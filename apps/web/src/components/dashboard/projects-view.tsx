/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { forgetProjectBundle, generateProjectName } from "@/lib/db";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuPortal,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { TextField, TextFieldInput } from "@/components/ui/text-field";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { toast } from "somoto";
import { For, Show, batch, createMemo, createResource, createSignal, onCleanup } from "solid-js";
import { useNavigate } from "@solidjs/router";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectPortal,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  DashboardCardMeta,
  DashboardCardButton,
  DashboardCardPreview,
  DashboardViewSection,
} from "./shared";
import { DashboardSearchPanel } from "./search-bar";
import { DashboardProjectsFolderBar } from "./projects-folder-bar";
import { projectRoute } from "@/hooks/use-project-route";
import { Icon } from "../ui/icon";
import { track } from "@/lib/analytics";
import {
  createProject,
  deleteProject,
  duplicateProject,
  ensureProjectsRoot,
  isDesktop,
  listProjects,
  projectKey,
  projectCoverKey,
  projectsRoot,
  readProjectCover,
  renameProject,
  type ProjectInfo,
} from "@/projects";

import type { ProjectSortOption } from "./types";

export function DashboardProjectsView() {
  const navigate = useNavigate();
  const [search, setSearch] = createSignal("");
  const [sort, setSort] = createSignal<ProjectSortOption>("last-viewed");
  const [projects, { refetch: refetchProjects }] = createResource(projectsRoot, () => listProjects());
  const [selectedProject, setSelectedProject] = createSignal<string | null>(null);
  const [creating, setCreating] = createSignal(false);
  const [pendingDelete, setPendingDelete] = createSignal<ProjectInfo | null>(null);
  const [deleting, setDeleting] = createSignal(false);
  const [renamingProject, setRenamingProject] = createSignal<string | null>(null);
  const [renameDraft, setRenameDraft] = createSignal("");

  const selectedSortOption = () =>
    SORT_OPTIONS.find((option) => option.id === sort()) ?? SORT_OPTIONS[0];

  const normalizedSearch = createMemo(() => search().trim().toLowerCase());

  const filteredProjects = createMemo(() => {
    const query = normalizedSearch();
    const entries = projects() ?? [];
    if (!query) return entries;

    return entries.filter((project) => project.displayName.toLowerCase().includes(query));
  });

  const sortedProjects = createMemo(() => {
    const sortMode = sort();
    const entries = [...filteredProjects()];

    if (sortMode === "alphabetical") {
      entries.sort((a, b) => a.displayName.localeCompare(b.displayName));
      return entries;
    }

    if (sortMode === "date-created") {
      entries.sort((a, b) => parseTimestamp(b.createdAt) - parseTimestamp(a.createdAt));
      return entries;
    }

    entries.sort((a, b) => parseTimestamp(b.modifiedAt) - parseTimestamp(a.modifiedAt));
    return entries;
  });

  const openProject = (project: ProjectInfo) => {
    if (renamingProject() === project.dir) return;

    track('project_opened');
    navigate(projectRoute(projectKey(project)));
  };

  const startRenaming = (project: ProjectInfo) => {
    batch(() => {
      setRenameDraft(project.displayName);
      setRenamingProject(project.dir);
    });
  };

  const handleDelete = async (project: ProjectInfo) => {
    try {
      await deleteProject(project.dir);
      forgetProjectBundle(project.id);
      track('project_deleted');
      setSelectedProject((current) => (current === project.dir ? null : current));
      refetchProjects();
    } catch (e) {
      toast.error("Failed to delete project", { description: (e as Error).message });
    }
  };

  const confirmDelete = async () => {
    const project = pendingDelete();
    if (!project || deleting()) return;

    setDeleting(true);
    try {
      await handleDelete(project);
    } finally {
      setDeleting(false);
      setPendingDelete(null);
    }
  };

  const handleDuplicate = async (project: ProjectInfo) => {
    try {
      await duplicateProject(project.dir);
      track('project_duplicated');
      refetchProjects();
    } catch (e) {
      toast.error("Failed to duplicate project", { description: (e as Error).message });
    }
  };

  const handleRenameInput = (event: InputEvent & { currentTarget: HTMLInputElement }) => {
    setRenameDraft(event.currentTarget.value);
  };

  // The input mounts from a context-menu selection, which restores focus to
  // the trigger on close — so the input must claim focus itself, after that.
  const handleRenameInputRef = (el: HTMLInputElement) => {
    queueMicrotask(() => {
      el.focus();
      el.select();
    });
  };

  const handleBlurRenameInput = () => {
    batch(() => {
      setRenamingProject(null);
      setRenameDraft("");
    });
    refetchProjects();
  };

  const handleKeyDownRenameInput = async (event: KeyboardEvent, project: ProjectInfo) => {
    const input = event.currentTarget as HTMLInputElement;
    const trimmedName = renameDraft()?.trim() ?? "";

    if (event.key === "Escape") {
      refetchProjects();
      setRenamingProject(null);
      setRenameDraft("");

      event.preventDefault();
      event.stopPropagation();
      input.blur();
    }

    if (event.key == "Enter") {
      event.preventDefault();
      event.stopPropagation();

      // The folder moves with the name, so the list is refetched below
      // rather than patched: every path in it has just changed.
      if (trimmedName.length > 0 && project.dir === renamingProject()) {
        try {
          await renameProject(project.dir, trimmedName);
        } catch (e) {
          toast.error("Failed to rename project", { description: (e as Error).message });
        }
      }

      refetchProjects();
      setRenamingProject(null);
      setRenameDraft("");

      input.blur();
    };
  };

  // New project is the one card a single click still acts on, so a double
  // click lands on it as two clicks — the guard keeps that from creating two
  // projects.
  const handleCreateProject = async () => {
    if (creating()) return;
    setCreating(true);

    try {
      if (!isDesktop()) {
        toast.error("Projects on disk are only available in the desktop app");
        return;
      }
      // Waits for the roots to come back from the database, and asks for one
      // when there is none to wait for.
      if (!(await ensureProjectsRoot())) return;

      const project = await createProject(generateProjectName());
      track('project_created');
      refetchProjects();
      openProject(project);
    } catch (e) {
      toast.error("Failed to create project", { description: (e as Error).message });
    } finally {
      setCreating(false);
    }
  };

  return (
    <DashboardSearchPanel
      value={search}
      onChange={setSearch}
      placeholder="Search in projects"
    >
      <DashboardViewSection
        class="pb-4"
        title="Recent projects"
        onBackgroundClick={() => setSelectedProject(null)}
        controls={
          <>
            <Select<(typeof SORT_OPTIONS)[number]>
              options={SORT_OPTIONS}
              value={selectedSortOption()}
              onChange={(option) => option && setSort(option.id)}
              optionValue="id"
              optionTextValue="label"
              itemComponent={(itemProps) => (
                <SelectItem item={itemProps.item}>
                  {itemProps.item.rawValue.label}
                </SelectItem>
              )}
            >
              <SelectTrigger aria-label="Sort projects">
                <SelectValue<(typeof SORT_OPTIONS)[number]>>
                  {(state) => state.selectedOption()?.label}
                </SelectValue>
              </SelectTrigger>
              <SelectPortal>
                <SelectContent />
              </SelectPortal>
            </Select>
          </>
        }
      >
        <DashboardCardButton onClick={handleCreateProject}>
          <DashboardCardPreview class="bg-overlay-soft group-hover:bg-overlay">
            <Icon
              name="plus-add"
              class="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 text-muted-foreground"
            />
          </DashboardCardPreview>
          <DashboardCardMeta title="New project" />
        </DashboardCardButton>
        <For each={sortedProjects().slice(0, MAX_VISIBLE_PROJECTS)}>
          {(project) => (
            <ContextMenu
              modal={false}
              onOpenChange={(open) => {
                if (open) setSelectedProject(project.dir);
              }}
            >
              <ContextMenuTrigger as="div" class="contents">
                <DashboardCardButton
                  active={selectedProject() === project.dir}
                  onClick={() => setSelectedProject(project.dir)}
                  onDoubleClick={() => openProject(project)}
                  onEscape={() => setSelectedProject(null)}
                  onDelete={() => setPendingDelete(project)}
                >
                  <DashboardCardPreview>
                    <ProjectThumbnail dir={project.dir} />
                  </DashboardCardPreview>
                  <div class="flex flex-col gap-1 px-2">
                    <div class="relative h-4 w-full">
                      <Show
                        when={renamingProject() === project.dir}
                        fallback={
                          <p class="min-w-0 truncate text-xs text-foreground">
                            {project.displayName}
                          </p>
                        }
                      >
                        <TextField class="contents">
                          <TextFieldInput
                            uiSize="compact"
                            type="text"
                            ref={handleRenameInputRef}
                            value={renameDraft()}
                            onInput={handleRenameInput}
                            onBlur={handleBlurRenameInput}
                            onKeyDown={(e: KeyboardEvent) => handleKeyDownRenameInput(e, project)}
                            placeholder="Project name"
                            aria-label="Project name"
                            class="absolute inset-x-0 top-1/2 h-5 w-full -translate-y-1/2 border border-ring bg-input px-1 py-0 ring-1 ring-inset ring-ring"
                          />
                        </TextField>
                      </Show>
                    </div>
                    <p class="min-w-0 truncate text-xs text-muted-foreground">
                      {formatEditedAt(project.modifiedAt)}
                    </p>
                  </div>
                </DashboardCardButton>
              </ContextMenuTrigger>
              <ContextMenuPortal>
                <ContextMenuContent class="w-45 gap-0">
                  <ContextMenuItem onSelect={() => openProject(project)}>
                    Open
                  </ContextMenuItem>
                  <ContextMenuSeparator class="my-2" />
                  <ContextMenuItem onSelect={() => startRenaming(project)}>
                    Rename
                  </ContextMenuItem>
                  <ContextMenuItem onSelect={() => handleDuplicate(project)}>
                    Duplicate
                  </ContextMenuItem>
                  <ContextMenuSeparator class="my-2" />
                  <ContextMenuItem onSelect={() => setPendingDelete(project)}>
                    Delete
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenuPortal>
            </ContextMenu>
          )}
        </For>
      </DashboardViewSection>
      <DashboardProjectsFolderBar />

      <AlertDialog
        open={pendingDelete() !== null}
        onOpenChange={(open) => {
          if (!open && !deleting()) setPendingDelete(null);
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete project</AlertDialogTitle>
            <AlertDialogDescription>
              {`"${pendingDelete()?.displayName ?? ""}" will be moved to the Trash. `}
              You can restore it from there until the Trash is emptied.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <Button
              variant="secondary"
              disabled={deleting()}
              onClick={() => setPendingDelete(null)}
            >
              Cancel
            </Button>
            <Button variant="destructive" disabled={deleting()} onClick={confirmDelete}>
              {deleting() ? "Deleting..." : "Delete"}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </DashboardSearchPanel>
  );
}

function ProjectThumbnail(props: { dir: string }) {
  const [cover] = createResource(
    () => projectCoverKey(props.dir),
    () => readProjectCover(props.dir),
  );

  // The URL the last cover was under is released as this one takes its place.
  const url = createMemo<string | null>((previous) => {
    if (previous) URL.revokeObjectURL(previous);
    const blob = cover();
    return blob ? URL.createObjectURL(blob) : null;
  }, null);

  onCleanup(() => {
    const current = url();
    if (current) URL.revokeObjectURL(current);
  });

  return (
    <Show when={url()}>
      <img
        src={url()!}
        alt=""
        class="h-full w-full object-cover"
        draggable={false}
      />
    </Show>
  );
}

const SORT_OPTIONS: Array<{ id: ProjectSortOption; label: string }> = [
  { id: "last-viewed", label: "Last modified" },
  { id: "alphabetical", label: "Alphabetical" },
  { id: "date-created", label: "Date created" },
];

const MAX_VISIBLE_PROJECTS = 11;

function parseTimestamp(value: string): number {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

function formatEditedAt(modifiedAt: string): string {
  const timestamp = parseTimestamp(modifiedAt);
  if (!timestamp) return "Edited just now";

  const elapsedMs = Date.now() - timestamp;
  if (elapsedMs < 60_000) return "Edited just now";

  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes < 60) return `Edited ${minutes} minute${minutes === 1 ? "" : "s"} ago`;

  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Edited ${hours} hour${hours === 1 ? "" : "s"} ago`;

  const days = Math.floor(hours / 24);
  if (days < 30) return `Edited ${days} day${days === 1 ? "" : "s"} ago`;

  const months = Math.floor(days / 30);
  if (months < 12) return `Edited ${months} month${months === 1 ? "" : "s"} ago`;

  const years = Math.floor(days / 365);
  return `Edited ${years} year${years === 1 ? "" : "s"} ago`;
}
