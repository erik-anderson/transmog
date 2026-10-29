import { observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WorkspaceElement } from "../workspace-element.js";
import type { HeaderRules } from "../header-rules/header-rules.js";
import "../header-rules/header-rules.js";
import { describeError } from "../utilities.js";
import initial from "../initial-state.json";

export class SessionReview extends WorkspaceElement {
  @observable sessionReviewAvailable = initial.sessionReviewAvailable;
  @observable sessionReviewName = initial.sessionReviewName;
  @observable sessionReviewEnded = initial.sessionReviewEnded;
  @observable sessionReviewKind = initial.sessionReviewKind;
  @observable sessionReviewError = initial.sessionReviewError;
  @observable sessionReviewMessage = initial.sessionReviewMessage;
  @observable sessionReviewBusy = initial.sessionReviewBusy;
  reviewDialog!: HTMLDialogElement;
  reviewButton!: HTMLButtonElement;
  headersEditor!: HeaderRules;
  textEditor!: HeaderRules;
  private unlisteners: UnlistenFn[] = [];
  protected async hydratedCallback(): Promise<void> {
    try {
      const state = await invoke<{ available: boolean; name?: string; accessEnded?: boolean }>(
        "mcp_review_state",
      );
      if (!this.isConnected || !state.available) {
        return;
      }
      this.sessionReviewAvailable = true;
      this.sessionReviewName = state.name ?? "Agent inspection";
      this.sessionReviewEnded = state.accessEnded ?? false;
      for (const event of [
        "mcp-automation-changed",
        "mcp-access-changed",
        "session-rule-close-requested",
      ]) {
        const stop = await listen(event, () => {
          if (event === "session-rule-close-requested") {
            this.sessionReviewError = "Save or discard your rule edits before closing this window.";
            this.open();
          } else {
            void this.refresh();
          }
        });
        if (this.isConnected) {
          this.unlisteners.push(stop);
        } else {
          stop();
        }
      }
    } catch {
      /* Ordinary capture viewers have no agent session. */
    }
  }
  async refresh(): Promise<void> {
    await Promise.all([this.headersEditor.refresh(), this.textEditor.refresh()]);
    const state = await invoke<{ accessEnded: boolean }>("mcp_review_state");
    this.sessionReviewEnded = state.accessEnded;
  }
  open(): void {
    if (!this.reviewDialog.open) {
      this.reviewDialog.showModal();
    }
    void this.refresh();
  }
  close(): void {
    this.reviewDialog.close();
    this.reviewButton.focus();
  }
  kind(value: string): void {
    this.sessionReviewKind = value;
  }
  async dirty(): Promise<void> {
    try {
      await invoke("mcp_review_dirty", {
        dirty: this.headersEditor.hrDirty || this.textEditor.hrDirty,
      });
    } catch (error) {
      this.sessionReviewError = describeError(error);
    }
  }
  async publish(): Promise<void> {
    const editor = this.sessionReviewKind === "headers" ? this.headersEditor : this.textEditor;
    const rule = editor.headerState?.rules.find((rule) => rule.id === editor.hrId);
    if (!rule || editor.hrDirty) {
      this.sessionReviewError = "Select a saved rule and resolve unsaved edits before publishing.";
      return;
    }
    this.sessionReviewBusy = true;
    this.sessionReviewError = "";
    this.sessionReviewMessage = "";
    try {
      await invoke("mcp_review_publish", { id: rule.id, revision: rule.revision });
      this.sessionReviewMessage =
        "Published to shared rules, disabled. Review and enable it in the main window.";
    } catch (error) {
      this.sessionReviewError = describeError(error);
    } finally {
      this.sessionReviewBusy = false;
    }
  }
  disconnectedCallback(): void {
    for (const stop of this.unlisteners) {
      stop();
    }
    super.disconnectedCallback();
  }
}
SessionReview.define("session-review");
