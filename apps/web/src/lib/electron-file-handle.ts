/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { mainBridge } from "@/lib/ipc";
import { MAIN_CHANNELS } from "@desktop/main-channels";

const INPUT_ID = "__electron-file-import-input__";
const INPUT_SELECTOR = `#${INPUT_ID}`;

export type SerializedElectronFileHandle = {
  kind: "electron-file";
  path: string;
  name: string;
};

let hiddenInput: HTMLInputElement | null = null;
let inFlight: Promise<unknown> = Promise.resolve();

function basename(path: string): string {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return idx === -1 ? path : path.slice(idx + 1);
}

function getHiddenInput(): HTMLInputElement {
  if (hiddenInput && document.body.contains(hiddenInput)) return hiddenInput;
  const input = document.createElement("input");
  input.type = "file";
  input.id = INPUT_ID;
  input.style.position = "fixed";
  input.style.left = "-10000px";
  input.style.top = "-10000px";
  input.style.width = "1px";
  input.style.height = "1px";
  input.style.opacity = "0";
  input.style.pointerEvents = "none";
  input.tabIndex = -1;
  input.setAttribute("aria-hidden", "true");
  document.body.appendChild(input);
  hiddenInput = input;
  return input;
}

async function materializeFile(absolutePath: string): Promise<File> {
  if (!window.desktop) {
    throw new Error("ElectronFileHandle requires the desktop bridge");
  }
  const input = getHiddenInput();
  await mainBridge.call(MAIN_CHANNELS.FILE_TRANSFER, {
    selector: INPUT_SELECTOR,
    absolutePath,
  });
  const file = input.files?.[0];
  // Reset so subsequent reads of `input.files[0]` aren't ambiguous if a
  // caller forgets to serialize through `inFlight`.
  input.value = "";
  if (!file) {
    throw new Error(`Failed to materialize file: ${absolutePath}`);
  }
  return file;
}

export class ElectronFileHandle {
  readonly kind = "file" as const;
  readonly name: string;
  readonly path: string;

  private filePromise: Promise<File> | null = null;

  public constructor(path: string, name?: string) {
    this.path = path;
    this.name = name ?? basename(path);
  }

  public static fromFile(path: string, file: File): ElectronFileHandle {
    const handle = new ElectronFileHandle(path, file.name);
    handle.filePromise = Promise.resolve(file);
    return handle;
  }

  public async getFile(): Promise<File> {
    if (!this.filePromise) {
      // Serialize CDP round-trips: every call shares the same hidden input,
      // and overlapping setFiles() calls would race on `input.files[0]`.
      const next = inFlight.then(() => materializeFile(this.path));
      inFlight = next.catch(() => {});
      this.filePromise = next;
      next.catch(this.invalidate.bind(this));
    }
    return this.filePromise;
  }

  public invalidate() {
    this.filePromise = null;
  }

  // FileSystemFileHandle shape — ElectronFileHandle is always "granted" because
  // the main process has full disk access.
  public async queryPermission(_descriptor?: any): Promise<PermissionState> {
    return "granted";
  }

  public async requestPermission(_descriptor?: any): Promise<PermissionState> {
    return "granted";
  }

  public toJSON(): SerializedElectronFileHandle {
    return { kind: "electron-file", path: this.path, name: this.name };
  }
}

export function isSerializedElectronFileHandle(value: unknown): value is SerializedElectronFileHandle {
  return (
    typeof value === "object" &&
    value !== null &&
    (value as { kind?: unknown }).kind === "electron-file"
  );
}

export function rehydrateElectronFileHandle(
  value: SerializedElectronFileHandle
): ElectronFileHandle {
  return new ElectronFileHandle(value.path, value.name);
}
