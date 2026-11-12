import { observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import initialState from "../initial-state.json";
import type { ProcessInspectionPolicy } from "../models.js";
import { WorkspaceElement } from "../workspace-element.js";
import { describeError } from "../utilities.js";

/** One independently checked policy draft inside the ordinary Settings save. */
export class ProcessInspectionSettings extends WorkspaceElement {
  @observable processMode: ProcessInspectionPolicy["mode"] = "all";
  @observable processNames: string[] = [];
  @observable processSuggestions: string[] = [];
  @observable processError = "";
  @observable processSuggestionStatus = initialState.processSuggestionStatus;
  @observable processLoading = false;
  processNameInput!: HTMLInputElement;
  private draft: ProcessInspectionPolicy = structuredClone(
    initialState.processInspection,
  ) as ProcessInspectionPolicy;
  baseline: ProcessInspectionPolicy = structuredClone(this.draft);

  populate(policy: ProcessInspectionPolicy): void {
    this.draft = structuredClone(policy);
    this.baseline = structuredClone(policy);
    this.processMode = policy.mode;
    this.updateNames();
    this.processNameInput.value = "";
    this.processError = "";
  }
  private updateNames(): void {
    this.processNames = this.processMode === "only" ? this.draft.include : this.draft.exclude;
  }
  chooseMode(event: Event): void {
    this.processMode = (event.target as HTMLSelectElement).value as ProcessInspectionPolicy["mode"];
    this.draft.mode = this.processMode;
    this.processError = "";
    this.updateNames();
    this.$emit("process-inspection-change");
  }
  editName(): void {
    this.processError = "";
    this.$emit("process-inspection-change");
  }
  nameKeyboard(event: KeyboardEvent): void {
    if (event.key === "Enter") {
      event.preventDefault();
      this.addName();
    }
  }
  addName(): boolean {
    let name = this.processNameInput.value.trim().toLowerCase();
    if (!name.includes(".")) {
      name += ".exe";
    }
    if (
      !name ||
      name === ".exe" ||
      /[\\/:*?"<>|\p{Cc}]/u.test(name) ||
      name.endsWith(".") ||
      new TextEncoder().encode(name).length > 260
    ) {
      this.processError =
        "Enter an executable name, such as chrome.exe, without a path or wildcard.";
      this.processNameInput.focus();
      return false;
    }
    if (this.processNames.some((saved) => saved.toLowerCase() === name.toLowerCase())) {
      this.processError = `${name} is already in this list.`;
      this.processNameInput.focus();
      return false;
    }
    if (this.processNames.length >= 256) {
      this.processError = "Use no more than 256 process names in each list.";
      return false;
    }
    const names = [...this.processNames, name];
    if (this.processMode === "only") {
      this.draft.include = names;
    } else {
      this.draft.exclude = names;
    }
    this.updateNames();
    this.processNameInput.value = "";
    this.processError = "";
    this.$emit("process-inspection-change");
    this.processNameInput.focus();
    return true;
  }
  removeName(name: string): void {
    const names = this.processNames.filter((saved) => saved !== name);
    if (this.processMode === "only") {
      this.draft.include = names;
    } else {
      this.draft.exclude = names;
    }
    this.updateNames();
    this.processError = "";
    this.$emit("process-inspection-change");
    this.processNameInput.focus();
  }
  policy(): ProcessInspectionPolicy {
    if (this.processMode !== "all" && this.processNameInput.value.trim() && !this.addName()) {
      throw new Error(this.processError);
    }
    if (this.processMode !== "all" && !this.processNames.length) {
      this.processError = "Add at least one process name, or choose All processes.";
      this.processNameInput.focus();
      throw new Error(this.processError);
    }
    return structuredClone(this.draft);
  }
  async refreshProcesses(): Promise<void> {
    if (this.processLoading) {
      return;
    }
    this.processLoading = true;
    this.processSuggestionStatus = "Loading running processes…";
    try {
      this.processSuggestions = await invoke<string[]>("list_running_processes");
      this.processSuggestionStatus = this.processSuggestions.length
        ? "Type a name or choose a running process suggestion."
        : "No running process names are available. Enter a name manually.";
    } catch (error: unknown) {
      this.processSuggestionStatus = describeError(error) + " Enter a name manually.";
    } finally {
      this.processLoading = false;
    }
  }
}

ProcessInspectionSettings.define("process-inspection-settings");
