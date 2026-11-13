import { WebUIElement, observable } from "@microsoft/webui-framework";
import { Channel, invoke } from "@tauri-apps/api/core";
import initialState from "../initial-state.json";
import { describeError } from "../utilities.js";

export interface UpdateStatus {
  phase: string;
  channel: string;
  currentVersion: string;
  version: string | null;
  notes: string;
  message: string;
  downloadedBytes: number;
  totalBytes: number | null;
  scheduled: boolean;
  suppressed: boolean;
  remindAfterUnixMs: number;
  lastCheckedUnixMs: number;
  automaticChecksEnabled: boolean;
}

/** The native coordinator owns eligibility, signed bytes, and installation consent. */
export class AppUpdates extends WebUIElement {
  @observable updateState: UpdateStatus = initialState.updateState;
  @observable updateBusy = initialState.updateBusy;
  @observable updateLastCheckedText = initialState.updateLastCheckedText;
  @observable updateProgressText = initialState.updateProgressText;
  @observable updateReminderText = initialState.updateReminderText;
  @observable updateError = initialState.updateError;
  @observable updatePreferenceBusy = initialState.updatePreferenceBusy;
  @observable updateCancelBusy = initialState.updateCancelBusy;
  private preferenceGeneration = 0;
  private preferenceRevision = 0;
  private downloadGeneration: number | null = null;
  private installWhenReadyGeneration: number | null = null;
  private generation = 0;
  private statusRevision = 0;
  private closeChannel: Channel<null> | undefined;

  protected hydratedCallback(): void {
    this.closeChannel = new Channel<null>();
    this.closeChannel.onmessage = () => this.$emit("install-update", { reopen: false });
    void invoke("watch_update_close", { channel: this.closeChannel }).catch(() => undefined);
    void this.check(false);
  }

  private apply(status: UpdateStatus, preferenceRevision = this.preferenceRevision): void {
    if (preferenceRevision !== this.preferenceRevision) {
      status = { ...status, automaticChecksEnabled: this.updateState.automaticChecksEnabled };
    }
    this.statusRevision++;
    this.updateState = {
      ...status,
      channel: status.channel || this.updateState.channel || "Release",
      automaticChecksEnabled: status.automaticChecksEnabled ?? true,
      message: status.message || "Updates have not been checked yet.",
    };
    this.updateLastCheckedText = status.lastCheckedUnixMs
      ? "Last successful check: " + new Date(status.lastCheckedUnixMs).toLocaleString()
      : "";
    this.$emit("update-state-change", status);
    this.updateProgressText = status.totalBytes
      ? `${Math.min(100, Math.floor((status.downloadedBytes / status.totalBytes) * 100))}% downloaded`
      : `${(status.downloadedBytes / 1024 / 1024).toFixed(1)} MiB downloaded`;
    this.updateReminderText =
      status.remindAfterUnixMs > Date.now()
        ? `Automatic prompts paused until ${new Date(status.remindAfterUnixMs).toLocaleDateString()}.`
        : "";
  }

  async setAutomaticChecks(event: Event): Promise<void> {
    const eventTarget = event?.currentTarget;
    const input = eventTarget as HTMLInputElement;
    if (this.updatePreferenceBusy) {
      return;
    }
    this.preferenceRevision++;
    const generation = ++this.preferenceGeneration,
      statusRevision = this.statusRevision;
    const current = (): boolean => generation === this.preferenceGeneration && this.isConnected;
    const accept = (status: UpdateStatus): void => {
      if (current()) {
        this.preferenceRevision++;
        this.apply(
          statusRevision === this.statusRevision
            ? status
            : { ...this.updateState, automaticChecksEnabled: status.automaticChecksEnabled },
        );
      }
    };
    this.updatePreferenceBusy = true;
    this.updateError = "";
    try {
      accept(await invoke<UpdateStatus>("set_automatic_update_checks", { enabled: input.checked }));
    } catch (error: unknown) {
      if (!current()) {
        return;
      }
      this.updateError = describeError(error);
      const status = await invoke<UpdateStatus>("update_status").catch(() => null);
      if (!current()) {
        return;
      }
      if (status) {
        accept(status);
      }
      input.checked = this.updateState.automaticChecksEnabled;
    } finally {
      if (generation === this.preferenceGeneration) {
        this.updatePreferenceBusy = false;
      }
    }
  }
  async check(manual = true): Promise<void> {
    if (this.updateBusy) {
      return;
    }
    const generation = ++this.generation;
    const preferenceRevision = this.preferenceRevision;
    this.updateBusy = true;
    this.updateError = "";
    try {
      const snapshot = await invoke<UpdateStatus>("update_status").catch(() => null);
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      const currentSnapshot = snapshot && {
        ...snapshot,
        automaticChecksEnabled:
          preferenceRevision === this.preferenceRevision
            ? snapshot.automaticChecksEnabled
            : this.updateState.automaticChecksEnabled,
      };
      if (currentSnapshot && !manual && currentSnapshot.automaticChecksEnabled === false) {
        this.apply(currentSnapshot);
        return;
      }
      if (currentSnapshot) {
        const track = currentSnapshot.channel === "Canary" ? "Canary" : "Stable";
        this.apply({
          ...currentSnapshot,
          phase: "checking",
          message: `Checking for ${track} updates…`,
        });
      }
      const status = await invoke<UpdateStatus>("check_updates", { manual });
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      this.apply(
        preferenceRevision === this.preferenceRevision
          ? status
          : { ...status, automaticChecksEnabled: this.updateState.automaticChecksEnabled },
      );
    } catch (error: unknown) {
      const status = await invoke<UpdateStatus>("update_status").catch(() => null);
      if (generation === this.generation && this.isConnected) {
        if (status) {
          this.apply(
            preferenceRevision === this.preferenceRevision
              ? status
              : { ...status, automaticChecksEnabled: this.updateState.automaticChecksEnabled },
          );
        }
        if (manual && describeError(error) !== status?.message) {
          this.updateError = describeError(error);
        }
      }
    } finally {
      if (generation === this.generation) {
        this.updateBusy = false;
      }
    }
  }

  async choose(afterSession: boolean): Promise<void> {
    if (this.updateBusy) {
      return;
    }
    const generation = ++this.generation;
    const preferenceRevision = this.preferenceRevision;
    this.downloadGeneration = generation;
    this.installWhenReadyGeneration = afterSession ? null : generation;
    this.updateBusy = true;
    this.updateError = "";
    this.apply({ ...this.updateState, phase: "downloading" });
    const progress = new Channel<UpdateStatus>();
    progress.onmessage = (status) => {
      if (generation === this.generation && this.isConnected) {
        this.apply(status, preferenceRevision);
      }
    };
    try {
      const status = await invoke<UpdateStatus>("download_update", { afterSession, progress });
      if (generation !== this.generation || !this.isConnected) {
        return;
      }
      this.apply(status, preferenceRevision);
      if (this.installWhenReadyGeneration === generation && status.phase === "ready") {
        this.$emit("install-update", { reopen: true });
      }
    } catch (error: unknown) {
      if (generation === this.generation && this.isConnected) {
        this.updateError = describeError(error);
        this.apply({ ...this.updateState, phase: "error" });
      }
    } finally {
      if (this.downloadGeneration === generation) {
        this.downloadGeneration = null;
        if (!this.updateCancelBusy) {
          this.updateBusy = false;
        }
      }
    }
  }

  async cancel(): Promise<void> {
    if (this.updateCancelBusy) {
      return;
    }
    let generation = this.generation;
    const preferenceRevision = this.preferenceRevision;
    // Continue observing the download until cancellation succeeds. A failed
    // request must not abandon its progress or allow overlapping native work.
    this.installWhenReadyGeneration = null;
    this.updateCancelBusy = true;
    this.updateError = "";
    this.updateBusy = true;
    try {
      const status = await invoke<UpdateStatus>("cancel_update");
      if (generation === this.generation && this.isConnected) {
        generation = ++this.generation;
        this.apply(status, preferenceRevision);
      }
    } catch (error: unknown) {
      if (generation === this.generation && this.isConnected) {
        this.updateError = describeError(error);
      }
    } finally {
      if (generation === this.generation) {
        this.updateBusy = this.downloadGeneration !== null;
        this.updateCancelBusy = false;
      }
    }
  }

  async remind(): Promise<void> {
    if (this.updateBusy) {
      return;
    }
    const generation = ++this.generation,
      preferenceRevision = this.preferenceRevision;
    this.updateBusy = true;
    this.updateError = "";
    try {
      const status = await invoke<UpdateStatus>("remind_update");
      if (generation === this.generation && this.isConnected) {
        this.apply(status, preferenceRevision);
      }
    } catch (error: unknown) {
      if (generation === this.generation && this.isConnected) {
        this.updateError = describeError(error);
      }
    } finally {
      if (generation === this.generation) {
        this.updateBusy = false;
      }
    }
  }

  async install(reopen: boolean): Promise<void> {
    if (this.updateBusy) {
      return;
    }
    const generation = ++this.generation,
      preferenceRevision = this.preferenceRevision;
    this.updateBusy = true;
    this.updateError = "";
    this.apply({
      ...this.updateState,
      phase: "installing",
      message: "Stopping capture and restoring Windows settings before installation…",
    });
    try {
      await invoke("install_update", { reopen });
    } catch (error: unknown) {
      const status = await invoke<UpdateStatus>("update_status").catch(() => ({
        ...this.updateState,
        phase: "error",
      }));
      if (generation === this.generation && this.isConnected) {
        this.updateError = describeError(error);
        this.apply(status, preferenceRevision);
      }
    } finally {
      if (generation === this.generation) {
        this.updateBusy = false;
      }
    }
  }

  installReady(): void {
    this.$emit("install-update", { reopen: true });
  }
  disconnectedCallback(): void {
    ++this.generation;
    ++this.preferenceRevision;
    ++this.preferenceGeneration;
    this.downloadGeneration = null;
    this.installWhenReadyGeneration = null;
    this.updateBusy = false;
    this.updateCancelBusy = false;
    this.updatePreferenceBusy = false;
    this.closeChannel = undefined;
    super.disconnectedCallback();
  }
}

AppUpdates.define("app-updates");
