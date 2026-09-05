/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { Match, Switch } from "solid-js";
import { toast } from "somoto";

import { Button } from "@/components/ui/button";
import { Icon } from "@/components/ui/icon";
import { pickProjectsRoot, projectsRoot } from "@/projects";
import { usePermissionState, type PermissionState } from "@/hooks/use-permission";

import {
  DashboardDividedStack,
  DashboardInfoActionRow,
  DashboardScrollView,
  DashboardSurfaceSection,
} from "./shared";

function DashboardProjectsFolderSection() {
  const handleChange = async () => {
    try {
      await pickProjectsRoot();
    } catch (e) {
      toast.error("Failed to choose projects folder", { description: (e as Error).message });
    }
  };

  return (
    <DashboardSurfaceSection title="Projects folder">
      <DashboardInfoActionRow
        title="Save projects to"
        leading={<Icon name="navigation.folder" class="text-foreground" />}
        description={projectsRoot() ?? "No folder selected"}
        action={
          <Button variant="secondary" onClick={handleChange}>
            Change...
          </Button>
        }
      />
    </DashboardSurfaceSection>
  );
}

type PermissionActionButtonProps = {
  state: PermissionState;
  pendingLabel: string;
  unavailableLabel?: string;
  onRequest(): void | Promise<void>;
};

function PermissionActionButton(props: PermissionActionButtonProps) {
  return (
    <Switch>
      <Match when={props.state === "granted"}>
        <Button variant="on">Granted</Button>
      </Match>
      <Match when={props.state === "denied"}>
        <Button variant="secondary" disabled>
          Denied
        </Button>
      </Match>
      <Match when={props.state === "unsupported"}>
        <Button variant="secondary" disabled>
          {props.unavailableLabel ?? "Unavailable"}
        </Button>
      </Match>
      <Match when={props.state === "prompt"}>
        <Button variant="secondary" onClick={props.onRequest}>
          {props.pendingLabel}
        </Button>
      </Match>
    </Switch>
  );
}

function DashboardPermissionsSection() {
  const clipboardState = usePermissionState("clipboard-read");
  const microphoneState = usePermissionState("microphone");
  const fontState = usePermissionState("local-fonts");
  const storageState = usePermissionState("persistent-storage");
  const notificationsState = usePermissionState("notifications");

  const handleClipboardRequest = async () => {
    try {
      await navigator.clipboard.readText();
    } catch {
      toast.error("Clipboard access denied");
    }
  };

  const handleFontsRequest = async () => {
    if (typeof window.queryLocalFonts !== "function") {
      toast.error("Local fonts are not supported in this browser");
      return;
    }
    try {
      await window.queryLocalFonts();
    } catch {
      toast.error("Local network access denied");
    }
  };

  const handleMicrophoneRequest = async () => {
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      stream.getTracks().forEach((track) => track.stop());
    } catch {
      toast.error("Microphone access denied");
    }
  };

  const handleStorageRequest = async () => {
    if (!navigator.storage?.persist) {
      toast.error("Persistent storage is not supported in this browser");
      return;
    }
    const granted = await navigator.storage.persist();
    if (!granted) toast.error("Persistent storage was not granted");
  };

  const handleNotificationsRequest = async () => {
    if (typeof Notification === "undefined") {
      toast.error("Notifications are not supported in this browser");
      return;
    }
    const result = await Notification.requestPermission();
    if (result !== "granted") toast.error("Notifications denied");
  };

  return (
    <DashboardSurfaceSection title="Permissions">
      <DashboardDividedStack>
        <DashboardInfoActionRow
          title="Clipboard access"
          description="Copy/paste text, timecodes, assets metadata."
          action={
            <PermissionActionButton
              state={clipboardState()}
              pendingLabel="Allow clipboard access..."
              onRequest={handleClipboardRequest}
            />
          }
        />
        <DashboardInfoActionRow
          title="Fonts access"
          description="Required to use installed fonts."
          action={
            <PermissionActionButton
              state={fontState()}
              pendingLabel="Allow fonts access..."
              onRequest={handleFontsRequest}
            />
          }
        />
        <DashboardInfoActionRow
          title="Microphone"
          description="Voiceover recording, live audio capture, speech-to-text."
          action={
            <PermissionActionButton
              state={microphoneState()}
              pendingLabel="Allow microphone..."
              onRequest={handleMicrophoneRequest}
            />
          }
        />
        <DashboardInfoActionRow
          title="Persistent storage"
          description="Prevents the browser from evicting your projects and imported assets under disk pressure."
          action={
            <PermissionActionButton
              state={window.chrome ? "granted" : storageState()}
              pendingLabel="Enable persistent storage..."
              onRequest={handleStorageRequest}
            />
          }
        />
        <DashboardInfoActionRow
          title="Notifications"
          description="Alerts when long-running exports and renders finish in the background."
          action={
            <PermissionActionButton
              state={notificationsState()}
              pendingLabel="Allow notifications..."
              onRequest={handleNotificationsRequest}
            />
          }
        />
      </DashboardDividedStack>
    </DashboardSurfaceSection>
  );
}

export function DashboardSettingsView() {
  return (
    <DashboardScrollView>
      <DashboardProjectsFolderSection />
      <DashboardPermissionsSection />
    </DashboardScrollView>
  );
}
