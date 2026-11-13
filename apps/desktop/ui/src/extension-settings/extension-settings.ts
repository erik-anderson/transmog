import initialState from "../initial-state.json";
import { captureFocus, captureRenderScope } from "../rendering.js";
import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WorkspaceElement } from "../workspace-element.js";
import { describeError } from "../utilities.js";
import type { ExtensionRow, ExtensionStatus } from "./models.js";
import { extensionRow } from "./presentation.js";

export class ExtensionSettings extends WorkspaceElement {
  async openDebugger(): Promise<void> {
    try {
      await invoke("open_debugger");
    } catch (error) {
      this.extensionsError = describeError(error);
    }
  }
  debugContribution(extensionId: string, visualizerId: string): void {
    this.$emit("extension-debug-request", { extensionId, visualizerId });
  }
  @attr({ mode: "boolean" }) active = false;
  @observable extensionsEntries: ExtensionRow[] = [];
  @observable extensionsBusy = initialState.extensionsBusy;
  @observable extensionsLoaded = initialState.extensionsLoaded;
  @observable extensionsText = initialState.extensionsText;
  @observable extensionsError = initialState.extensionsError;
  @observable extensionsWarning = initialState.extensionsWarning;
  @observable extensionsBlocked = initialState.extensionsBlocked;
  private unlisten: UnlistenFn | undefined;
  private observing = false;
  private listenerGeneration = 0;
  private refreshAgain = false;
  private generation = 0;
  activeChanged(): void {
    if (this.active && this.isConnected) {
      this.watch();
      if (this.extensionsBusy) {
        this.refreshAgain = true;
      } else {
        void this.refresh();
      }
    } else {
      this.unwatch();
    }
  }
  protected hydratedCallback(): void {
    if (this.active) {
      this.watch();
      void this.refresh();
    }
  }
  private watch(): void {
    if (this.observing) {
      return;
    }
    this.observing = true;
    const generation = ++this.listenerGeneration;
    void listen("extensions-changed", () => {
      if (!this.active) {
        return;
      }
      if (this.extensionsBusy) {
        this.refreshAgain = true;
      } else {
        void this.refresh(true);
      }
    })
      .then((unlisten) => {
        if (generation === this.listenerGeneration && this.isConnected && this.active) {
          this.unlisten = unlisten;
        } else {
          void Promise.resolve(unlisten()).catch(() => undefined);
        }
      })
      .catch(() => {
        if (generation === this.listenerGeneration) {
          this.observing = false;
          this.extensionsError =
            "Extension changes could not be observed. Use Refresh to check the current state.";
        }
      });
  }
  private unwatch(): void {
    this.listenerGeneration++;
    this.observing = false;
    const unlisten = this.unlisten;
    this.unlisten = undefined;
    if (unlisten) {
      void Promise.resolve(unlisten()).catch(() => undefined);
    }
  }
  disconnectedCallback(): void {
    this.generation++;
    this.unwatch();
    super.disconnectedCallback();
  }
  private refreshQueued(): void {
    if (!this.refreshAgain) {
      return;
    }
    this.refreshAgain = false;
    if (this.active) {
      void this.refresh(true);
    }
  }
  private async accept(status: ExtensionStatus): Promise<void> {
    const focus = captureFocus(this);
    const surface = this.shadowRoot ?? this;
    const root = surface.getRootNode();
    const focused =
      root instanceof Document || root instanceof ShadowRoot ? root.activeElement : null;
    const previousFocus =
      focused instanceof HTMLElement && surface.contains(focused) ? focused : null;
    const summaries: HTMLElement[] = [];
    for (
      let parent = previousFocus?.parentElement;
      parent && surface.contains(parent);
      parent = parent.parentElement
    ) {
      if (parent instanceof HTMLDetailsElement && parent.firstElementChild instanceof HTMLElement) {
        summaries.push(parent.firstElementChild);
      }
    }
    this.extensionsEntries = status.entries.map(extensionRow);
    this.extensionsWarning = status.warning ?? "";
    this.extensionsBlocked = status.managementBlocked ?? Boolean(status.warning);
    this.extensionsLoaded = true;
    if (previousFocus) {
      const currentScope = captureRenderScope(this, () => [this.extensionsEntries, this.active]);
      await this.$whenRendered();
      const current =
        root instanceof Document || root instanceof ShadowRoot ? root.activeElement : null;
      if (
        currentScope() &&
        focus() &&
        !previousFocus.isConnected &&
        (!current || current === this.ownerDocument.body)
      ) {
        summaries
          .find((summary) => summary.isConnected && summary.getClientRects().length > 0)
          ?.focus();
      }
    }
  }
  async refresh(preserveFeedback = false): Promise<void> {
    if (this.extensionsBusy) {
      return;
    }
    const generation = ++this.generation;
    this.extensionsBusy = true;
    if (!preserveFeedback) {
      this.extensionsError = "";
      this.extensionsText = "";
    }
    try {
      const status = await invoke<ExtensionStatus>("extension_status");
      if (generation === this.generation && this.isConnected && !this.refreshAgain) {
        await this.accept(status);
      }
    } catch (error: unknown) {
      if (generation === this.generation && !this.refreshAgain) {
        this.extensionsError = describeError(error);
      }
    } finally {
      if (generation === this.generation) {
        this.extensionsBusy = false;
        this.refreshQueued();
      }
    }
  }
  private async change(
    command: string,
    args: Record<string, unknown>,
    progress: string,
    complete: string,
    rollback?: ExtensionRow[],
  ): Promise<void> {
    if (this.extensionsBusy) {
      return;
    }
    const generation = ++this.generation;
    this.extensionsBusy = true;
    this.extensionsError = "";
    this.extensionsText = progress;
    try {
      const status = await invoke<ExtensionStatus | null>(command, args);
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      if (status) {
        await this.accept(status);
        this.extensionsText = complete;
      } else {
        this.extensionsText = "";
      }
    } catch (error: unknown) {
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      this.extensionsError = describeError(error);
      this.extensionsText = "";
      if (rollback) {
        this.extensionsEntries = rollback;
      }
      // A file cleanup error can follow a successful catalog change. Read the
      // authoritative state instead of restoring an obsolete checkbox/list.
      try {
        const status = await invoke<ExtensionStatus>("extension_status");
        if (generation === this.generation && this.isConnected) {
          await this.accept(status);
        }
      } catch {
        /* Keep the last valid inventory and the original actionable error. */
      }
      if (this.active) {
        await this.showError("Extension change could not finish", this.extensionsError);
      }
    } finally {
      if (generation === this.generation) {
        this.extensionsBusy = false;
        this.refreshQueued();
      }
    }
  }
  install(): void {
    void this.change(
      "install_extension",
      {},
      "Installing extension…",
      "Extension installed. Choose it from a matching message to visualize.",
    );
  }
  unpacked(): void {
    void this.change(
      "load_unpacked_extension",
      {},
      "Choosing an unpacked extension…",
      "Unpacked extension registered. Its source files stay in their folder.",
    );
  }
  enabled(id: string, event: Event): void {
    if (this.extensionsBusy) {
      return;
    }
    const enabled = (event.target as HTMLInputElement).checked;
    const previous = this.extensionsEntries;
    this.extensionsEntries = previous.map((entry) =>
      entry.id === id ? { ...entry, enabled } : entry,
    );
    void this.change(
      "enable_extension",
      { id, enabled },
      "Saving extension settings…",
      enabled
        ? "Extension enabled."
        : "Extension disabled. Its active visualizers have stopped in every window.",
      previous,
    );
  }
  hotReload(id: string, event: Event): void {
    if (this.extensionsBusy) {
      return;
    }
    const enabled = (event.target as HTMLInputElement).checked;
    const previous = this.extensionsEntries;
    this.extensionsEntries = previous.map((entry) =>
      entry.id === id ? { ...entry, hotReload: enabled } : entry,
    );
    void this.change(
      "extension_hot_reload",
      { id, enabled },
      "Saving development settings…",
      enabled ? "Hot reload enabled for active development visualizers." : "Hot reload disabled.",
      previous,
    );
  }
  async reload(id: string): Promise<void> {
    if (this.extensionsBusy) {
      return;
    }
    const generation = ++this.generation;
    this.extensionsBusy = true;
    this.extensionsError = "";
    this.extensionsText = "Requesting reload…";
    try {
      const result = await invoke<{ activeInstances: number }>("reload_extension", { id });
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      this.extensionsText = result.activeInstances
        ? "Reload requested for active visualizers in every window. Each keeps its last good result if replacement fails."
        : "No active visualizers to reload. Open one from a matching message to use the current source files.";
    } catch (error: unknown) {
      if (generation === this.generation && this.isConnected) {
        this.extensionsError = describeError(error);
        this.extensionsText = "";
        if (this.active) {
          await this.showError("Extension reload could not be requested", this.extensionsError);
        }
      }
    } finally {
      if (generation === this.generation) {
        this.extensionsBusy = false;
        this.refreshQueued();
      }
    }
  }
  async package(id: string): Promise<void> {
    if (this.extensionsBusy) {
      return;
    }
    const generation = ++this.generation;
    this.extensionsBusy = true;
    this.extensionsError = "";
    this.extensionsText = "Packaging extension…";
    try {
      const result = await invoke<{ path: string; digest: string; bytes: number } | null>(
        "package_extension",
        { id },
      );
      if (generation === this.generation && this.isConnected) {
        this.extensionsText = result ? "Extension packaged: " + result.path : "";
      }
    } catch (error: unknown) {
      if (generation === this.generation && this.isConnected) {
        this.extensionsError = describeError(error);
        this.extensionsText = "";
        if (this.active) {
          await this.showError("Extension could not be packaged", this.extensionsError);
        }
      }
    } finally {
      if (generation === this.generation) {
        this.extensionsBusy = false;
        this.refreshQueued();
      }
    }
  }
  async exportFolder(id: string): Promise<void> {
    if (this.extensionsBusy) {
      return;
    }
    const generation = ++this.generation;
    this.extensionsBusy = true;
    this.extensionsError = "";
    this.extensionsText = "Saving extension content…";
    try {
      const result = await invoke<{ path: string; files: number } | null>("export_extension", {
        id,
      });
      if (generation === this.generation && this.isConnected) {
        this.extensionsText = result
          ? "Extension saved: " +
            result.path +
            ". To load a separate development copy, change its manifest ID first."
          : "";
      }
    } catch (error: unknown) {
      if (generation === this.generation && this.isConnected) {
        this.extensionsError = describeError(error);
        this.extensionsText = "";
        if (this.active) {
          await this.showError("Extension could not be saved", this.extensionsError);
        }
      }
    } finally {
      if (generation === this.generation) {
        this.extensionsBusy = false;
        this.refreshQueued();
      }
    }
  }
  uninstall(id: string): void {
    void this.change(
      "uninstall_extension",
      { id },
      "Removing extension…",
      "Extension removed from Transmog. Unpacked source files are unchanged.",
    );
  }
  dismiss(): void {
    this.extensionsText = "";
    this.extensionsError = "";
  }
}
ExtensionSettings.define("extension-settings");
