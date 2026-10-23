import { observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import { describeError } from "../utilities.js";
import { bytesToHex, decodeBodyBase64 } from "../response-body-codec.js";
import type {
  AutomationStatus,
  AutomationRule,
  ResponseSet,
  ResponseSetVersion,
  ResponseAssetInspection,
} from "../models.js";

export interface ResponseSetRestoreRequest {
  id: string;
  incarnation: string;
  versionId: string;
  versionRevision: number;
  resolve: (allowed: boolean, isCurrent?: () => boolean) => void;
}
export interface ResponseSetRestored {
  id: string;
  incarnation: string;
  focus: boolean;
}
export interface ResponseSetDeleted {
  id: string;
  incarnation: string;
  focus: boolean;
}
export interface ResponseSetDemoRequest {
  mode: "save" | "list";
}
type Owner = { id: string; incarnation: string; revision: number };
type ConfirmKind = "restore" | "delete-version" | "delete-set";

function describeRules(rules: AutomationRule[]): string {
  const included = rules.filter((rule) => rule.enabled !== false),
    names = included.slice(0, 3).map((rule) => rule.displayName || "Unnamed response");
  return `${included.length} included response${included.length === 1 ? "" : "s"}${names.length ? ": " + names.join(", ") : ""}${included.length > names.length ? ", …" : ""}`;
}

/** Live participation and optional milestones are independent of the current draft. */
export class ResponseSetLifecycle extends WorkspaceElement {
  @observable status: AutomationStatus | null = null;
  @observable setId = "";
  @observable baseRevision = 0;
  @observable baseIncarnation = "";
  @observable currentOutdated = false;
  @observable visible = false;
  @observable dirty = false;
  @observable locked = false;
  @observable editorPending = false;
  @observable tourStep = 0;
  @observable ownerName = "";
  @observable stateText = "";
  @observable applyLabel = "";
  @observable canApply = false;
  @observable canPause = false;
  @observable canResume = false;
  @observable globallyPaused = false;
  @observable undoDescription = "";
  @observable pending = false;
  @observable lifecycleError = "";
  @observable lifecycleMessage = "";
  @observable versionRows: Array<{ id: string; name: string; summary: string; revision: number }> =
    [];
  @observable versionName = "";
  @observable versionError = "";
  @observable versionMessage = "";
  @observable viewedVersionName = "";
  @observable viewedVersionRules: Array<{
    id: string;
    name: string;
    method: string;
    url: string;
    included: boolean;
    definition: string;
  }> = [];
  @observable selectedVersionResponseId = "";
  @observable versionResponseName = "";
  @observable versionResponseLoading = false;
  @observable versionResponseError = "";
  @observable versionResponseStatus = "";
  @observable versionResponseHeaders = "";
  @observable versionResponseBody = "";
  @observable versionResponseDescription = "";
  @observable confirmTitle = "";
  @observable confirmBody = "";
  @observable confirmLabel = "";
  @observable confirmError = "";
  @observable confirmCompleted = false;
  @observable confirmProgress = "";
  @observable overlapRows: string[] = [];
  @observable overlapAcknowledged = false;
  @observable overlapError = "";
  @observable hasPage = false;
  liveButton!: HTMLButtonElement;
  pauseButton!: HTMLButtonElement;
  resumeButton!: HTMLButtonElement;
  moreButton!: HTMLButtonElement;
  moreMenu!: HTMLElement;
  tryButton!: HTMLButtonElement;
  tryMenu!: HTMLElement;
  versionsDialog!: HTMLDialogElement;
  versionNameInput!: HTMLInputElement;
  versionViewDialog!: HTMLDialogElement;
  confirmDialog!: HTMLDialogElement;
  overlapDialog!: HTMLDialogElement;
  overlapCheckbox!: HTMLInputElement;
  private epoch = 0;
  private selectionKey = "";
  private returnFocus: HTMLElement | null = null;
  private confirmReturnFocus: HTMLElement | null = null;
  private viewReturnFocus: HTMLElement | null = null;
  private confirmKind: ConfirmKind = "delete-set";
  private confirmingOwner: Owner | null = null;
  private confirmingVersion: ResponseSetVersion | null = null;
  private viewedSnapshot: ResponseSetVersion | null = null;
  private viewEpoch = 0;
  private overlapsResolve: ((allowed: boolean) => void) | null = null;

  statusChanged(): void {
    this.render();
  }
  setIdChanged(): void {
    this.render();
  }
  baseRevisionChanged(): void {
    this.render();
  }
  baseIncarnationChanged(): void {
    this.render();
  }
  visibleChanged(): void {
    if (!this.visible) {
      this.leave();
    }
  }
  private owner(): ResponseSet | undefined {
    return this.status?.responseSets?.find((set) => set.id === this.setId);
  }
  private context(owner: Owner, epoch = this.epoch): boolean {
    const current = this.owner();
    return (
      this.isConnected &&
      this.visible &&
      epoch === this.epoch &&
      current?.id === owner.id &&
      current.incarnation === owner.incarnation
    );
  }
  private available(): boolean {
    return (
      this.visible &&
      !this.locked &&
      !this.editorPending &&
      !this.tourStep &&
      !this.pending &&
      !!this.owner()
    );
  }
  private active(): HTMLElement | null {
    let element: Element | null = document.activeElement;
    while (element?.shadowRoot?.activeElement) {
      element = element.shadowRoot.activeElement;
    }
    return element instanceof HTMLElement ? element : null;
  }
  private restoreFocus(element: HTMLElement | null): void {
    if (this.visible && element?.isConnected && element.getClientRects().length) {
      element.focus({ preventScroll: true });
    }
  }
  private restoreLiveFocus(): void {
    this.$flushUpdates();
    this.restoreFocus(
      this.canApply
        ? this.liveButton
        : this.canPause
          ? this.pauseButton
          : this.canResume
            ? this.resumeButton
            : this.moreButton,
    );
  }
  private closeMenu(menu: HTMLElement, button: HTMLButtonElement): void {
    if (menu.matches(":popover-open")) {
      menu.hidePopover();
    }
    this.restoreFocus(button);
  }
  private render(): void {
    const set = this.owner(),
      key = set ? set.id + "@" + set.incarnation : "";
    if (key !== this.selectionKey) {
      this.leave();
      this.selectionKey = key;
      this.lifecycleError = "";
      this.lifecycleMessage = "";
    }
    this.ownerName = set?.name ?? "";
    this.hasPage = !!set?.previewPage;
    this.currentOutdated =
      !!set && (set.revision !== this.baseRevision || set.incarnation !== this.baseIncarnation);
    const changed = !!set?.applied && set.applied.revision !== set.revision;
    this.stateText = !set?.applied
      ? "Not applied to live traffic"
      : set.paused
        ? "Paused"
        : "Active on live traffic";
    if (changed) {
      this.stateText = (set?.paused ? "Paused" : "Active") + " · Saved changes not applied";
    }
    this.canApply = !!set && (!set.applied || changed);
    this.applyLabel = !set?.applied
      ? "Use on live traffic"
      : set.paused
        ? "Apply and resume"
        : "Apply changes";
    this.canPause = !!set?.applied && !set.paused;
    this.canResume = !!set?.applied && set.paused && !changed;
    this.globallyPaused = this.status?.autoresponsesEnabled === false;
    const undo = set?.undoApply;
    this.undoDescription =
      undo && undo.generation === this.status?.generation
        ? undo.applied
          ? `Restore saved revision ${undo.applied.revision} (${describeRules(undo.applied.rules)})${undo.paused ? ", paused for this set" : ", active for this set"}.`
          : "Stop using this set on live traffic; no previous applied responses."
        : "";
    this.versionRows = (set?.versions ?? []).map((version) => ({
      id: version.id,
      name: version.name,
      revision: version.revision,
      summary: describeRules(version.rules),
    }));
  }
  private publish(status: AutomationStatus): void {
    this.$emit("set-status", status);
    if (status.generation >= (this.status?.generation ?? 0)) {
      this.status = status;
    }
  }
  private async latest(
    owner: Owner,
    epoch: number,
    exactRevision = false,
  ): Promise<AutomationStatus> {
    const status = await invoke<AutomationStatus>("automation_status");
    if (!this.context(owner, epoch)) {
      throw new Error("The selected response set changed.");
    }
    const current = status.responseSets?.find((set) => set.id === owner.id);
    if (
      !current ||
      current.incarnation !== owner.incarnation ||
      (exactRevision && current.revision !== owner.revision)
    ) {
      throw new Error(
        "This response set changed. Refresh its saved current collection and try again. Your draft is preserved.",
      );
    }
    this.publish(status);
    return status;
  }
  private failure(error: unknown, owner: Owner, epoch: number, title: string): void {
    const message = describeError(error);
    if (this.context(owner, epoch)) {
      this.lifecycleError = message;
    } else {
      this.showNotice(title, message, null, null);
    }
  }
  async apply(): Promise<void> {
    const set = this.owner();
    if (!set || !this.available() || this.dirty || this.currentOutdated || !this.canApply) {
      return;
    }
    const owner = { id: set.id, incarnation: set.incarnation, revision: set.revision },
      epoch = this.epoch,
      source = this.active(),
      resume = !!set.applied && set.paused;
    this.pending = true;
    this.lifecycleError = "";
    this.lifecycleMessage = "";
    try {
      const status = await this.latest(owner, epoch, true),
        overlaps = await invoke<string[]>("response_set_overlaps", {
          id: owner.id,
          revision: owner.revision,
        });
      if (!this.context(owner, epoch) || this.dirty) {
        return;
      }
      if (overlaps.length) {
        // Opening a delayed warning may not take newly moved focus.
        if (this.active() !== source) {
          this.lifecycleMessage = "Review overlaps by choosing " + this.applyLabel + " again.";
          return;
        }
        this.overlapRows = overlaps;
        this.overlapAcknowledged = false;
        this.overlapError = "";
        this.returnFocus = source;
        this.$flushUpdates();
        this.overlapCheckbox.checked = false;
        this.overlapDialog.showModal();
        const allowed = await new Promise<boolean>((resolve) => {
          this.overlapsResolve = resolve;
        });
        if (!allowed) {
          return;
        }
      }
      if (!this.context(owner, epoch) || this.dirty) {
        return;
      }
      const result = await invoke<AutomationStatus>("change_response_set", {
        generation: status.generation,
        change: {
          action: "apply",
          id: owner.id,
          expectedRevision: owner.revision,
          acknowledgeOverlaps: overlaps.length > 0,
          resume,
        },
      });
      const ownsFocus = this.active() === source;
      this.publish(result);
      if (this.context(owner, epoch)) {
        this.lifecycleMessage = resume
          ? "Applied saved changes and resumed this set."
          : "Applied saved changes to live traffic.";
        if (ownsFocus) {
          this.restoreLiveFocus();
        }
      }
    } catch (error) {
      this.failure(error, owner, epoch, "Live response changes failed");
    } finally {
      this.pending = false;
    }
  }
  acknowledgeOverlap(event: Event): void {
    this.overlapAcknowledged = (event.currentTarget as HTMLInputElement).checked;
  }
  finishOverlap(allowed: boolean, event?: Event, restore = true): void {
    event?.preventDefault();
    if (allowed && !this.overlapAcknowledged) {
      return;
    }
    const resolve = this.overlapsResolve;
    this.overlapsResolve = null;
    this.overlapDialog?.close();
    if (restore && resolve) {
      this.restoreFocus(this.returnFocus);
    }
    resolve?.(allowed);
  }
  async participation(action: "pause" | "resume" | "undo"): Promise<void> {
    const set = this.owner();
    if (!set || !this.available()) {
      return;
    }
    if (
      (action === "pause" && !this.canPause) ||
      (action === "resume" && !this.canResume) ||
      (action === "undo" && !this.undoDescription)
    ) {
      return;
    }
    const owner = { id: set.id, incarnation: set.incarnation, revision: set.revision },
      epoch = this.epoch,
      undoGeneration = set.undoApply?.generation,
      source = this.active();
    this.pending = true;
    this.lifecycleError = "";
    this.lifecycleMessage = "";
    try {
      const status = await this.latest(owner, epoch);
      if (action === "undo" && status.generation !== undoGeneration) {
        throw new Error("Undo is no longer available because automation changed.");
      }
      const change =
        action === "undo"
          ? { action: "undo-apply", id: set.id }
          : { action: "pause", id: set.id, paused: action === "pause" };
      const result = await invoke<AutomationStatus>("change_response_set", {
          generation: status.generation,
          change,
        }),
        ownsFocus = this.active() === source;
      this.publish(result);
      if (this.context(owner, epoch)) {
        this.lifecycleMessage =
          action === "undo"
            ? "Restored the previous applied responses and per-set pause state."
            : action === "pause"
              ? "Paused this set on live traffic."
              : "Resumed this set on live traffic.";
        if (ownsFocus) {
          this.restoreLiveFocus();
        }
      }
    } catch (error) {
      this.failure(error, owner, epoch, "Live response change failed");
    } finally {
      this.pending = false;
    }
  }
  showVersions(): void {
    if (!this.available()) {
      return;
    }
    this.closeMenu(this.moreMenu, this.moreButton);
    this.returnFocus = this.moreButton;
    this.versionName = "";
    this.versionError = "";
    this.versionMessage = "";
    this.$flushUpdates();
    this.versionsDialog.showModal();
    this.versionNameInput.value = "";
    this.versionNameInput.focus();
  }
  closeVersions(event?: Event): void {
    event?.preventDefault();
    this.versionsDialog?.close();
    this.restoreFocus(this.returnFocus);
  }
  versionNameInputChanged(event: Event): void {
    this.versionName = (event.currentTarget as HTMLInputElement).value;
  }
  async saveVersion(event: Event): Promise<void> {
    event.preventDefault();
    const set = this.owner();
    if (!set || !this.available() || this.currentOutdated) {
      return;
    }
    const name = this.versionName.trim();
    if (
      !name ||
      new TextEncoder().encode(name).length > 128 ||
      // eslint-disable-next-line no-control-regex -- User-visible names cannot contain control characters.
      /[\u0000-\u001f\u007f]/.test(name)
    ) {
      this.versionError =
        "Enter a version name of at most 128 UTF-8 bytes, without control characters.";
      return;
    }
    const owner = { id: set.id, incarnation: set.incarnation, revision: set.revision },
      epoch = this.epoch;
    this.pending = true;
    this.versionError = "";
    try {
      const status = await this.latest(owner, epoch, true);
      const result = await invoke<AutomationStatus>("change_response_set", {
        generation: status.generation,
        change: {
          action: "save-version",
          id: owner.id,
          expectedRevision: owner.revision,
          versionId: "version-" + crypto.randomUUID().replaceAll("-", ""),
          name,
        },
      });
      this.publish(result);
      if (this.context(owner, epoch)) {
        this.versionMessage = `Saved named version “${name}” from saved revision ${owner.revision}.`;
      }
    } catch (error) {
      if (this.context(owner, epoch)) {
        this.versionError = describeError(error);
      } else {
        this.showNotice("Version save failed", describeError(error), null, null);
      }
    } finally {
      this.pending = false;
    }
  }
  viewVersion(id: string): void {
    const version = this.owner()?.versions.find((version) => version.id === id);
    if (!version || !this.available()) {
      return;
    }
    this.viewEpoch++;
    this.selectedVersionResponseId = "";
    this.versionResponseLoading = false;
    this.versionResponseError = "";
    this.viewReturnFocus = this.active();
    this.viewedVersionName = version.name;
    this.viewedSnapshot = structuredClone(version);
    this.viewedVersionRules = version.rules.map((rule) => ({
      id: rule.id,
      name: rule.displayName ?? "Unnamed response",
      method: rule.matcher.method ?? "ANY",
      url:
        rule.matcher.url?.kind === "exact"
          ? rule.matcher.url.value
          : rule.matcher.url?.kind === "pattern"
            ? rule.matcher.url.value.address
            : rule.matcher.url?.kind === "regex"
              ? rule.matcher.url.value.pattern
              : (rule.matcher.host ?? "Any URL"),
      included: rule.enabled !== false,
      definition: JSON.stringify(rule, null, 2),
    }));
    this.$flushUpdates();
    this.versionViewDialog.showModal();
    if (version.rules[0]) {
      void this.inspectVersionResponse(version.rules[0].id);
    }
  }
  closeVersionView(event?: Event): void {
    event?.preventDefault();
    this.viewEpoch++;
    this.versionResponseLoading = false;
    this.viewedSnapshot = null;
    this.versionViewDialog?.close();
    this.restoreFocus(this.viewReturnFocus);
  }
  async inspectVersionResponse(id: string): Promise<void> {
    const rule = this.viewedSnapshot?.rules.find((rule) => rule.id === id);
    if (!rule || !this.versionViewDialog.open) {
      return;
    }
    const epoch = ++this.viewEpoch;
    this.selectedVersionResponseId = id;
    this.versionResponseName = rule.displayName ?? "Unnamed response";
    this.versionResponseLoading = true;
    this.versionResponseError = "";
    this.versionResponseBody = "";
    this.versionResponseHeaders = "";
    this.versionResponseStatus = "";
    this.versionResponseDescription = "";
    try {
      if (!rule.request.responseAsset) {
        throw new Error("The saved response body is unavailable.");
      }
      const inspection = await invoke<ResponseAssetInspection>("inspect_response_asset", {
        reference: rule.request.responseAsset,
      });
      if (epoch !== this.viewEpoch || !this.versionViewDialog.open) {
        return;
      }
      const asset = inspection.asset;
      this.versionResponseStatus = `HTTP ${asset.status} · ${asset.mediaType ?? "Media type unavailable"} · ${asset.bodyBytes.toLocaleString()} saved bytes`;
      const headerText = (bytes: number[]) => {
        try {
          return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(bytes)).replace(
            // eslint-disable-next-line no-control-regex -- Display captured header controls as escaped bytes.
            /[\u0000-\u001f\u007f]/g,
            (char) => "\\x" + char.charCodeAt(0).toString(16).padStart(2, "0"),
          );
        } catch {
          return bytes
            .map((byte) =>
              byte >= 32 && byte < 127
                ? String.fromCharCode(byte)
                : "\\x" + byte.toString(16).padStart(2, "0"),
            )
            .join("");
        }
      };
      this.versionResponseHeaders = asset.headers
        .map((header) => headerText(header.name) + ": " + headerText(header.value))
        .join("\n");
      if (asset.linkedBody) {
        this.versionResponseDescription = `Linked file: ${asset.linkedBody.path}. This named version retains the file reference; external file edits can change its live response bytes.`;
        return;
      }
      if (inspection.bytesBase64 !== null) {
        const bytes = decodeBodyBase64(inspection.bytesBase64),
          binary = inspection.textEncoding === null,
          limited = binary ? bytes.length > 65536 : inspection.display.length > 65536;
        this.versionResponseBody = binary
          ? bytesToHex(bytes.slice(0, 65536))
          : inspection.display.slice(0, 65536);
        this.versionResponseDescription = `${binary ? "Hex bytes" : "Text · " + inspection.textEncoding}${limited ? " · Preview limited to the first 65,536 " + (binary ? "bytes" : "characters") : ""}. Read-only saved response.`;
      } else {
        this.versionResponseBody = inspection.display.slice(0, 65536);
        this.versionResponseDescription = inspection.display
          ? "Bounded original-byte preview. " +
            inspection.explanation.split(" Serve the complete body")[0]
          : "Saved body unavailable. Headers and status remain available.";
      }
    } catch (error) {
      if (epoch === this.viewEpoch) {
        this.versionResponseError = describeError(error);
      }
    } finally {
      if (epoch === this.viewEpoch) {
        this.versionResponseLoading = false;
      }
    }
  }
  ask(kind: ConfirmKind, versionId = ""): void {
    const set = this.owner();
    if (!set || !this.available()) {
      return;
    }
    const version = set.versions.find((item) => item.id === versionId);
    if (kind !== "delete-set" && !version) {
      return;
    }
    if (kind === "delete-set") {
      this.closeMenu(this.moreMenu, this.moreButton);
    }
    this.confirmKind = kind;
    this.confirmingOwner = { id: set.id, incarnation: set.incarnation, revision: set.revision };
    this.confirmingVersion = version ? structuredClone(version) : null;
    this.confirmReturnFocus = this.active();
    this.confirmError = "";
    this.confirmProgress = "";
    this.confirmCompleted = false;
    this.confirmTitle =
      kind === "delete-set"
        ? `Delete ${set.name}?`
        : kind === "restore"
          ? `Restore ${version!.name} to the current set?`
          : `Delete version ${version!.name}?`;
    this.confirmBody =
      kind === "delete-set"
        ? `This deletes its responses, notes, named versions, and saved demos.${set.applied ? " This also stops using this set’s changes on live traffic." : ""}${this.dirty ? " Unsaved changes to this set will be lost." : ""}`
        : kind === "restore"
          ? "This replaces the current responses with this saved version. Applied live responses and saved demos stay as they are."
          : "This deletes only this named version. Current responses, applied live responses, and saved demos stay as they are.";
    this.confirmLabel =
      kind === "delete-set"
        ? "Delete response set"
        : kind === "restore"
          ? "Restore to current set"
          : "Delete version";
    this.$flushUpdates();
    this.confirmDialog.showModal();
  }
  private restoreConfirmFocus(): void {
    this.$flushUpdates();
    const source = this.confirmReturnFocus;
    if (source?.isConnected && source.getClientRects().length) {
      this.restoreFocus(source);
    } else if (this.versionsDialog?.open) {
      this.restoreFocus(this.versionNameInput);
    }
  }
  cancelConfirm(event?: Event): void {
    event?.preventDefault();
    this.confirmDialog?.close();
    this.restoreConfirmFocus();
  }
  async confirm(): Promise<void> {
    const owner = this.confirmingOwner,
      version = this.confirmingVersion,
      kind = this.confirmKind;
    if (!owner || this.pending || this.confirmCompleted || !this.context(owner)) {
      return;
    }
    const epoch = this.epoch,
      source = this.active();
    this.pending = true;
    this.confirmError = "";
    this.confirmProgress = "";
    try {
      let unchanged = () => true;
      if (kind === "restore") {
        const allowed = await new Promise<boolean>((resolve) =>
          this.$emit("set-restore-request", {
            id: owner.id,
            incarnation: owner.incarnation,
            versionId: version!.id,
            versionRevision: version!.revision,
            resolve: (allowed, isCurrent) => {
              unchanged = isCurrent ?? (() => true);
              resolve(allowed);
            },
          } satisfies ResponseSetRestoreRequest),
        );
        if (!allowed || !this.context(owner, epoch)) {
          return;
        }
      }
      const status = await this.latest(owner, epoch),
        saved = status.responseSets!.find((set) => set.id === owner.id)!;
      if (
        kind !== "delete-set" &&
        !saved.versions.some(
          (item) => item.id === version!.id && item.revision === version!.revision,
        )
      ) {
        throw new Error(
          "This named version is no longer available. Close this dialog and choose another version.",
        );
      }
      const change =
        kind === "restore"
          ? {
              action: "restore-version",
              id: owner.id,
              expectedRevision: saved.revision,
              versionId: version!.id,
            }
          : kind === "delete-version"
            ? { action: "delete-version", id: owner.id, versionId: version!.id }
            : { action: "delete", id: owner.id };
      const result = await invoke<AutomationStatus>("change_response_set", {
        generation: status.generation,
        change,
      });
      const current = this.context(owner, epoch),
        ownsFocus = this.active() === source;
      this.publish(result);
      if (kind === "delete-set") {
        this.$emit("set-deleted", {
          id: owner.id,
          incarnation: owner.incarnation,
          focus: current && ownsFocus,
        } satisfies ResponseSetDeleted);
        if (current) {
          this.leave();
        }
        return;
      }
      if (!current) {
        return;
      }
      this.confirmCompleted = true;
      this.confirmProgress =
        kind === "restore"
          ? "Restored this version to the current set. Live traffic is unchanged."
          : "Deleted this named version.";
      if (kind === "restore" && unchanged()) {
        this.$emit("set-restored", {
          id: owner.id,
          incarnation: owner.incarnation,
          focus: ownsFocus,
        } satisfies ResponseSetRestored);
      } else if (kind === "restore") {
        this.lifecycleMessage =
          "Restored the saved version. Your newer unsaved draft is preserved; reopen the set to view the restored responses.";
      }
      if (ownsFocus) {
        this.confirmDialog.close();
        this.restoreConfirmFocus();
      }
    } catch (error) {
      if (this.context(owner, epoch)) {
        this.confirmError = describeError(error);
      } else {
        this.showNotice("Response set change failed", describeError(error), null, null);
      }
    } finally {
      this.pending = false;
    }
  }
  tryAction(action: "live" | "preview" | "save-demo" | "demos"): void {
    if (!this.available()) {
      return;
    }
    this.closeMenu(this.tryMenu, this.tryButton);
    if (action === "live") {
      void this.openLive();
      return;
    }
    if (action === "preview") {
      if (!this.dirty) {
        this.$emit("set-preview-request", undefined);
      }
      return;
    }
    if (action === "save-demo" && this.dirty) {
      return;
    }
    this.$emit("set-demo-request", {
      mode: action === "save-demo" ? "save" : "list",
    } satisfies ResponseSetDemoRequest);
  }
  demos(): void {
    if (!this.available()) {
      return;
    }
    this.closeMenu(this.moreMenu, this.moreButton);
    this.$emit("set-demo-request", { mode: "list" } satisfies ResponseSetDemoRequest);
  }
  help(): void {
    this.closeMenu(this.moreMenu, this.moreButton);
    this.$emit("set-help-request", undefined);
  }
  async openLive(): Promise<void> {
    const set = this.owner();
    if (!set?.previewPage || !this.available()) {
      return;
    }
    const owner = { id: set.id, incarnation: set.incarnation, revision: set.revision },
      epoch = this.epoch;
    this.pending = true;
    this.lifecycleError = "";
    try {
      await invoke("open_response_set_live_page", {
        id: set.id,
        incarnation: set.incarnation,
        url: set.previewPage.url,
      });
      if (this.context(owner, epoch)) {
        this.lifecycleMessage =
          "Opened the live page. Response changes apply when requests pass through Transmog.";
      }
    } catch (error) {
      this.failure(error, owner, epoch, "Live page could not open");
    } finally {
      this.pending = false;
    }
  }
  dismissFeedback(): void {
    this.lifecycleMessage = "";
    this.lifecycleError = "";
  }
  keyboard(event: KeyboardEvent): void {
    if ((event.target as HTMLElement)?.closest("dialog")) {
      event.stopPropagation();
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
      }
    }
  }
  leave(): void {
    this.epoch++;
    this.viewEpoch++;
    this.finishOverlap(false, undefined, false);
    this.versionsDialog?.close();
    this.versionViewDialog?.close();
    this.confirmDialog?.close();
    if (this.moreMenu?.matches(":popover-open")) {
      this.moreMenu.hidePopover();
    }
    if (this.tryMenu?.matches(":popover-open")) {
      this.tryMenu.hidePopover();
    }
  }
  disconnectedCallback(): void {
    this.leave();
    super.disconnectedCallback();
  }
}
ResponseSetLifecycle.define("response-set-lifecycle");
