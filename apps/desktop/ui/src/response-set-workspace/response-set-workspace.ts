import { observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import "../response-set-tour/response-set-tour.js";
import "../response-set-lifecycle/response-set-lifecycle.js";
import type {
  ResponseSetLifecycle,
  ResponseSetDemoRequest,
} from "../response-set-lifecycle/response-set-lifecycle.js";
import type { ResponseSetTour } from "../response-set-tour/response-set-tour.js";
import { pageIdentity } from "../response-set-page-picker/response-set-page-picker.js";
import { describeError } from "../utilities.js";
import type {
  AutomationStatus,
  AutomationRule,
  ResponseSet,
  ExperimentSelection,
  ResponseSetGuideHistory,
  ResponseSetGuideUpdate,
  ExperimentPreviewPages,
  SelectedResponse,
} from "../models.js";

const tourSteps = [
  {
    title: "Keep a debugging task together",
    copy: "Give this set a name and notes so you can pick up the work later.",
  },
  {
    title: "Collect your response changes",
    copy: "Add captured responses, existing rules, or responses you create yourself.",
  },
  {
    title: "Save your work",
    copy: "Save changes as you go. Keep a named version when you want a milestone.",
  },
  {
    title: "Try it now or demonstrate it later",
    copy: "Apply changes to live traffic, preview a capture, or save a demo for later.",
  },
];

export class ResponseSetWorkspace extends WorkspaceElement {
  @observable overviewVisible = false;
  @observable preferencesReady = false;
  @observable guideHistory: ResponseSetGuideHistory = "not-offered";
  @observable guidePending = false;
  @observable guideError = "";
  @observable invitationVisible = false;
  @observable helpOpen = false;
  @observable tourStep = 0;
  @observable tourContext = "";
  @observable stepNumber = 0;
  @observable stepTitle = "";
  @observable stepCopy = "";
  @observable stepLast = false;
  lifecycle!: ResponseSetLifecycle;
  get guideHelpButton(): HTMLButtonElement {
    return this.lifecycle.moreButton;
  }
  guideStartButton!: HTMLButtonElement;
  guideHelpPanel!: HTMLDialogElement;
  tourPanel!: ResponseSetTour;
  private guideEpoch = 0;
  private offerChecked = false;
  overviewVisibleChanged(): void {
    if (!this.overviewVisible) {
      this.leaveGuide();
      this.pageEpoch++;
    } else {
      void this.offerGuide();
      void this.refreshPage();
    }
  }
  preferencesReadyChanged(): void {
    void this.offerGuide();
  }
  guideHistoryChanged(): void {
    void this.offerGuide();
  }
  busyChanged(): void {
    if (this.busy) {
      this.leaveGuide();
    } else {
      void this.offerGuide();
    }
  }
  disconnectedCallback(): void {
    this.leaveGuide();
    super.disconnectedCallback();
  }
  private guideAvailable(): boolean {
    return (
      this.isConnected &&
      this.overviewVisible &&
      this.preferencesReady &&
      !!this.setId &&
      !this.busy
    );
  }
  private rememberGuide(update: ResponseSetGuideUpdate): void {
    this.guideHistory = update.history;
    this.$emit("workspace-change", {
      patch: { responseSetGuide: update.history },
      committed: false,
    });
  }
  private async offerGuide(): Promise<void> {
    if (this.offerChecked || this.guidePending || !this.guideAvailable()) {
      return;
    }
    this.offerChecked = true;
    if (this.guideHistory !== "not-offered") {
      return;
    }
    const epoch = this.guideEpoch;
    this.guidePending = true;
    try {
      const update = await invoke<ResponseSetGuideUpdate>("update_response_set_guide", {
        action: "offer",
      });
      this.rememberGuide(update);
      if (epoch === this.guideEpoch && this.guideAvailable()) {
        this.invitationVisible = update.offered;
      }
    } catch (error) {
      if (epoch === this.guideEpoch) {
        this.guideError = "Tour invitation could not be remembered: " + describeError(error);
      }
    } finally {
      this.guidePending = false;
    }
  }
  showGuideHelp(): void {
    if (!this.overviewVisible) {
      return;
    }
    this.guideEpoch++;
    this.tourStep = 0;
    this.helpOpen = true;
    this.guideError = "";
    this.$flushUpdates();
    this.guideHelpPanel.scrollIntoView({ block: "nearest" });
    this.guideHelpPanel.focus({ preventScroll: true });
  }
  closeGuideHelp(): void {
    this.guideEpoch++;
    this.helpOpen = false;
    this.restoreGuideFocus();
  }
  dismissGuideInvitation(): void {
    this.guideEpoch++;
    this.invitationVisible = false;
    this.restoreGuideFocus();
  }
  async startGuide(): Promise<void> {
    if (this.guidePending || !this.guideAvailable()) {
      return;
    }
    // Pending Start stays focusable (aria-disabled in the template): native
    // disabling blurs it and would invalidate its own focus ownership below.
    const epoch = ++this.guideEpoch,
      selection = this.selectionEpoch,
      source =
        this.getRootNode() instanceof ShadowRoot
          ? (this.getRootNode() as ShadowRoot).activeElement
          : document.activeElement;
    this.guidePending = true;
    this.guideError = "";
    try {
      const update = await invoke<ResponseSetGuideUpdate>("update_response_set_guide", {
        action: "start",
      });
      this.rememberGuide(update);
      if (
        epoch !== this.guideEpoch ||
        selection !== this.selectionEpoch ||
        !this.guideAvailable()
      ) {
        return;
      }
      const root = this.getRootNode(),
        active = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
      // A delayed preference write must not remove help underneath newly moved focus.
      if (active !== source) {
        return;
      }
      this.helpOpen = false;
      this.invitationVisible = false;
      this.setTourStep(1);
      this.tourPanel.focusNext();
    } catch (error) {
      if (epoch === this.guideEpoch) {
        this.guideError = "Tour could not start: " + describeError(error);
      }
    } finally {
      this.guidePending = false;
    }
  }
  private setTourStep(step: number): void {
    this.tourStep = step;
    const copy = tourSteps[step - 1]!;
    this.stepNumber = step;
    this.stepTitle = copy.title;
    this.stepCopy = copy.copy;
    this.stepLast = step === 4;
    this.updateTourContext();
    this.$flushUpdates();
    this.tourPanel.scrollIntoView({ block: "nearest" });
  }
  nextTourStep(): void {
    if (this.tourStep === 4) {
      this.endGuide();
    } else if (this.tourStep) {
      this.setTourStep(this.tourStep + 1);
      this.tourPanel.focusNext();
    }
  }
  previousTourStep(): void {
    if (this.tourStep > 1) {
      this.setTourStep(this.tourStep - 1);
      this.tourPanel.focusNext();
    }
  }
  endGuide(): void {
    this.guideEpoch++;
    this.tourStep = 0;
    this.restoreGuideFocus();
  }
  private leaveGuide(): void {
    this.guideEpoch++;
    this.tourStep = 0;
    this.helpOpen = false;
    this.invitationVisible = false;
  }
  dismissGuideForNavigation(): void {
    this.leaveGuide();
  }
  private restoreGuideFocus(): void {
    this.$flushUpdates();
    if (this.overviewVisible && this.isConnected) {
      this.guideHelpButton.scrollIntoView({ block: "nearest" });
      this.guideHelpButton.focus({ preventScroll: true });
    }
  }
  guideKeyboard(event: KeyboardEvent): void {
    if (event.key === "Escape" && (this.tourStep || this.helpOpen)) {
      event.preventDefault();
      event.stopPropagation();
      if (this.tourStep) {
        this.endGuide();
      } else {
        this.closeGuideHelp();
      }
    } else if (
      this.tourStep &&
      (event.key === "Delete" ||
        ((event.ctrlKey || event.metaKey) &&
          ["a", "z", "y", "s"].includes(event.key.toLowerCase())))
    ) {
      event.preventDefault();
      event.stopPropagation();
    } else if (this.helpOpen && this.guideHelpPanel.contains(event.target as Node)) {
      event.stopPropagation();
      if (
        event.key === "Delete" ||
        ((event.ctrlKey || event.metaKey) && ["a", "z", "y", "s"].includes(event.key.toLowerCase()))
      ) {
        event.preventDefault();
      }
    }
  }
  private updateTourContext(): void {
    this.tourContext =
      this.tourStep === 2 && !this.draftRules.length
        ? "This set is empty. You can finish the tour without adding a response."
        : this.tourStep === 3
          ? "Save changes keeps the current collection. More… → Versions… keeps a named milestone."
          : this.tourStep === 4
            ? "Live use, captured previews, and saved demos are separate choices."
            : "";
  }
  @observable status: AutomationStatus | null = null;
  @observable setId = "";
  @observable setName = "";
  @observable setNotes = "";
  @observable search = "";
  @observable rows: Array<{
    id: string;
    name: string;
    notes: string;
    count: string;
    state: string;
    modified: string;
  }> = [];
  @observable ruleRows: Array<{
    id: string;
    name: string;
    method: string;
    url: string;
    response: string;
    included: boolean;
    selected: boolean;
    first: boolean;
    last: boolean;
  }> = [];
  @observable selectedRuleId = "";
  @observable dirty = false;
  @observable busy = false;
  @observable editorLoading = false;
  @observable fileBusy = false;
  @observable addingResponse = false;
  @observable responsePending = false;
  editorLoadingChanged(): void {
    this.responsePending = this.editorLoading || this.fileBusy || this.addingResponse;
  }
  fileBusyChanged(): void {
    this.responsePending = this.editorLoading || this.fileBusy || this.addingResponse;
  }
  addingResponseChanged(): void {
    this.responsePending = this.editorLoading || this.fileBusy || this.addingResponse;
  }
  @observable error = "";
  @observable message = "";
  @observable liveState = "";
  @observable createPending = false;
  @observable createCompleted = false;
  @observable createError = "";
  @observable previewReportAvailable = false;
  @observable previewReportSetId = "";
  @observable pageUrl = "";
  @observable pageIdentity = "";
  @observable pageAvailability = "";
  @observable draftRules: AutomationRule[] = [];
  newSetButton!: HTMLButtonElement;
  backButton!: HTMLButtonElement;
  nameInput!: HTMLInputElement;
  notesInput!: HTMLTextAreaElement;
  createDialog!: HTMLDialogElement;
  createName!: HTMLInputElement;
  createNotes!: HTMLTextAreaElement;
  createButton!: HTMLButtonElement;
  createCancel!: HTMLButtonElement;
  addButton!: HTMLButtonElement;
  addMenu!: HTMLElement;
  private selectionEpoch = 0;
  private pageEpoch = 0;
  private createEpoch = 0;
  private createdSetId = "";
  private createSource: SelectedResponse | null = null;
  private baseline = "";
  @observable private draftRevision = 0;
  @observable private draftIncarnation = "";
  private editorDirty = false;
  statusChanged(): void {
    this.renderLibrary();
    if (this.setId) {
      void this.refreshPage();
    }
  }
  current(): ResponseSet | undefined {
    return this.status?.responseSets?.find((set) => set.id === this.setId);
  }
  currentSelection(): ExperimentSelection {
    return {
      experimentId: this.setId,
      incarnation: this.draftIncarnation,
      variantId: "",
      revision: this.draftRevision,
    };
  }
  private values(): string {
    return JSON.stringify({ name: this.setName, notes: this.setNotes, rules: this.draftRules });
  }
  private updateDirty(): void {
    this.dirty = this.editorDirty || this.values() !== this.baseline;
  }
  markEditorDirty(dirty: boolean): void {
    this.editorDirty = dirty;
    this.updateDirty();
  }
  metadataChanged(event: Event): void {
    const field = event.currentTarget as HTMLInputElement;
    if (field === this.nameInput) {
      this.setName = field.value;
    } else {
      this.setNotes = field.value;
    }
    this.error = "";
    this.updateDirty();
  }
  renderLibrary(): void {
    const query = this.search.toLocaleLowerCase();
    this.rows = (this.status?.responseSets ?? [])
      .filter((set) => (set.name + " " + set.notes).toLocaleLowerCase().includes(query))
      .map((set) => ({
        id: set.id,
        name: set.name,
        notes: set.notes,
        count: `${set.rules.length} response${set.rules.length === 1 ? "" : "s"}`,
        state: this.state(set),
        modified: new Date(set.modifiedAt).toLocaleString(),
      }));
    this.liveState = this.current() ? this.state(this.current()!) : "";
  }
  private state(set: ResponseSet): string {
    if (!set.applied) {
      return "Not applied to live traffic";
    }
    const changed = set.applied.revision !== set.revision;
    return (
      (set.paused ? "Paused" : "Active on live traffic") +
      (changed ? " · Saved changes not applied" : "")
    );
  }
  searchInput(event: Event): void {
    this.search = (event.currentTarget as HTMLInputElement).value;
    this.renderLibrary();
  }
  requestOpen(id: string, source: SelectedResponse | null = null): void {
    this.$emit("set-open", { id, source });
  }
  open(id: string): void {
    const set = this.status?.responseSets?.find((set) => set.id === id);
    if (!set) {
      return;
    }
    if (id !== this.setId) {
      this.leaveGuide();
    }
    this.selectionEpoch++;
    this.setId = id;
    this.draftRevision = set.revision;
    this.draftIncarnation = set.incarnation;
    this.setName = set.name;
    this.setNotes = set.notes;
    this.draftRules = structuredClone(set.rules);
    this.selectedRuleId = "";
    this.editorDirty = false;
    this.baseline = this.values();
    this.dirty = false;
    this.error = "";
    this.message = "";
    this.renderRules();
    this.renderLibrary();
    this.$flushUpdates();
    this.nameInput.value = this.setName;
    this.notesInput.value = this.setNotes;
    void this.refreshPage();
    void this.offerGuide();
  }
  close(focus = true): void {
    this.selectionEpoch++;
    this.pageEpoch++;
    this.leaveGuide();
    this.setId = "";
    this.selectedRuleId = "";
    this.draftRules = [];
    this.dirty = false;
    this.editorDirty = false;
    this.$flushUpdates();
    if (focus) {
      this.newSetButton.focus({ preventScroll: true });
    }
  }
  requestBack(): void {
    this.$emit("set-back", undefined);
  }
  edit(id: string): void {
    if (!this.busy && !this.responsePending && !this.tourStep) {
      this.$emit("set-edit", { ruleId: id });
    }
  }
  select(id: string): void {
    this.selectedRuleId = id;
    this.renderRules();
  }
  renderRules(): void {
    this.ruleRows = this.draftRules.map((rule, index) => ({
      id: rule.id,
      name: rule.displayName ?? "New response",
      method: rule.matcher.method ?? "ANY",
      url:
        rule.matcher.url?.kind === "exact"
          ? rule.matcher.url.value
          : rule.matcher.url?.kind === "pattern"
            ? rule.matcher.url.value.address
            : (rule.matcher.host ?? "Any URL"),
      response: rule.request.responseAsset ? "Saved response" : "New response",
      included: rule.enabled !== false,
      selected: rule.id === this.selectedRuleId,
      first: index === 0,
      last: index === this.draftRules.length - 1,
    }));
    this.updateDirty();
  }
  stageRule(rule: AutomationRule): void {
    const found = this.draftRules.some((item) => item.id === rule.id);
    this.draftRules = found
      ? this.draftRules.map((item) => (item.id === rule.id ? rule : item))
      : [...this.draftRules, rule];
    this.renderRules();
  }
  updateRuleName(id: string, name: string): void {
    if (!id) {
      return;
    }
    const label = name || "New response";
    if (this.draftRules.find((rule) => rule.id === id)?.displayName !== label) {
      this.draftRules = this.draftRules.map((rule) =>
        rule.id === id ? { ...rule, displayName: label } : rule,
      );
      this.renderRules();
    }
  }
  changeRule(id: string, action: string): void {
    if (this.busy || this.responsePending || this.tourStep) {
      return;
    }
    if (action === "remove") {
      this.$emit("set-remove", { ruleId: id });
      return;
    }
    if (action === "toggle") {
      this.draftRules = this.draftRules.map((rule) =>
        rule.id === id ? { ...rule, enabled: rule.enabled === false } : rule,
      );
    } else {
      const next = [...this.draftRules],
        index = next.findIndex((rule) => rule.id === id),
        target = index + (action === "up" ? -1 : 1);
      if (index < 0 || target < 0 || target >= next.length) {
        return;
      }
      [next[index], next[target]] = [next[target]!, next[index]!];
      this.draftRules = next;
    }
    this.renderRules();
  }
  removeRule(id: string): void {
    this.draftRules = this.draftRules.filter((rule) => rule.id !== id);
    this.renderRules();
  }
  rowKeyboard(id: string, event: KeyboardEvent): void {
    if (event.target instanceof HTMLInputElement) {
      return;
    }
    if (event.altKey && ["ArrowUp", "ArrowDown"].includes(event.key)) {
      event.preventDefault();
      this.changeRule(id, event.key === "ArrowUp" ? "up" : "down");
    } else if (event.key === "Delete") {
      event.preventDefault();
      this.changeRule(id, "remove");
    }
  }
  add(source: string): void {
    this.addMenu.hidePopover();
    this.addButton.focus({ preventScroll: true });
    this.$emit("set-add", { source });
  }
  save(): void {
    if (!this.tourStep) {
      this.$emit("set-save", undefined);
    }
  }
  discard(): void {
    if (!this.tourStep) {
      this.$emit("set-discard", undefined);
    }
  }
  keyboard(event: KeyboardEvent): void {
    this.guideKeyboard(event);
    if (event.defaultPrevented) {
      return;
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "s" &&
      this.setId &&
      !this.createDialog.open
    ) {
      event.preventDefault();
      event.stopPropagation();
      this.save();
    }
  }
  showCreate(source: SelectedResponse | null = null): void {
    if (this.busy || this.createPending) {
      return;
    }
    this.createEpoch++;
    this.createCompleted = false;
    this.createdSetId = "";
    this.createSource = source;
    this.createError = "";
    this.createName.value = "";
    this.createNotes.value = "";
    this.createDialog.showModal();
    this.createName.focus();
  }
  openCreated(): void {
    if (!this.createdSetId) {
      return;
    }
    this.createDialog.close();
    this.requestOpen(this.createdSetId, this.createSource);
  }
  closeCreate(event?: Event): void {
    event?.preventDefault();
    this.createEpoch++;
    this.createDialog.close();
    this.newSetButton.focus({ preventScroll: true });
  }
  async create(event: Event): Promise<void> {
    event.preventDefault();
    if (this.createPending || this.createCompleted || !this.status) {
      return;
    }
    const name = this.createName.value.trim(),
      notes = this.createNotes.value;
    if (!this.validate(name, notes)) {
      this.createError = this.error;
      this.error = "";
      return;
    }
    const epoch = this.createEpoch,
      id = "set-" + crypto.randomUUID().replaceAll("-", ""),
      source = (this.getRootNode() as ShadowRoot).activeElement;
    this.createPending = true;
    this.createError = "";
    try {
      const status = await invoke<AutomationStatus>("change_response_set", {
        generation: this.status.generation,
        change: { action: "create", id, name, notes, rules: [] },
      });
      this.$emit("set-status", status);
      if (status.generation >= (this.status?.generation ?? 0)) {
        this.status = status;
      }
      if (epoch === this.createEpoch && this.createDialog.open) {
        const focused = (this.getRootNode() as ShadowRoot).activeElement,
          ownsFocus = focused === source;
        this.createdSetId = id;
        if (ownsFocus) {
          this.createDialog.close();
          this.requestOpen(id, this.createSource);
        } else {
          if (focused === this.createButton) {
            this.createCancel.focus({ preventScroll: true });
          }
          this.createCompleted = true;
        }
      }
    } catch (error) {
      if (epoch === this.createEpoch) {
        this.createError = describeError(error);
      }
    } finally {
      this.createPending = false;
    }
  }
  validate(name = this.setName.trim(), notes = this.setNotes): boolean {
    const bytes = new TextEncoder();
    this.error = !name
      ? "Enter a response set name."
      : bytes.encode(name).length > 128
        ? "The response set name must be at most 128 UTF-8 bytes."
        : bytes.encode(notes).length > 16384
          ? "Notes must be at most 16,384 UTF-8 bytes."
          : "";
    return !this.error;
  }
  saved(status: AutomationStatus): void {
    this.status = status;
    const set = this.current();
    if (!set) {
      return;
    }
    this.draftRevision = set.revision;
    this.draftIncarnation = set.incarnation;
    this.setName = set.name;
    this.setNotes = set.notes;
    this.draftRules = structuredClone(set.rules);
    this.editorDirty = false;
    this.baseline = this.values();
    this.dirty = false;
    this.message = "Saved";
    this.renderRules();
    this.renderLibrary();
  }
  previewContext(): () => boolean {
    const epoch = this.selectionEpoch,
      selection = JSON.stringify(this.currentSelection());
    return () =>
      this.overviewVisible &&
      !!this.setId &&
      !this.dirty &&
      !this.busy &&
      epoch === this.selectionEpoch &&
      selection === JSON.stringify(this.currentSelection());
  }
  preview(): void {
    if (!this.dirty) {
      this.$emit("set-preview", {
        selection: this.currentSelection(),
        ruleId: this.draftRules.find((rule) => rule.enabled !== false)?.id ?? null,
      });
    }
  }
  changePage(): void {
    const id = this.setId,
      current = this.previewContext();
    this.$emit("page-association-request", { id, isCurrent: () => id === this.setId && current() });
  }
  demoRequest(event: CustomEvent<ResponseSetDemoRequest>): void {
    event.stopPropagation();
    this.$emit("set-demo-request", event.detail);
  }
  showPreviewDiagnostics(): void {
    this.$emit("preview-report-request", undefined);
  }
  async refreshPage(): Promise<void> {
    const id = this.setId,
      page = this.current()?.previewPage,
      epoch = ++this.pageEpoch;
    this.pageUrl = page?.url ?? "";
    this.pageIdentity = page ? pageIdentity(page) : "";
    this.pageAvailability = page ? "Checking capture…" : "";
    if (!page || !this.overviewVisible) {
      return;
    }
    try {
      const result = await invoke<ExperimentPreviewPages>("experiment_preview_pages", {
        id,
        selectedId: null,
        includePages: false,
      });
      if (epoch === this.pageEpoch && id === this.setId) {
        this.pageAvailability = result.available
          ? "Retained preview capture"
          : "Capture unavailable";
      }
    } catch {
      if (epoch === this.pageEpoch) {
        this.pageAvailability = "Capture availability could not be checked";
      }
    }
  }
}
customElements.define("response-set-workspace", ResponseSetWorkspace);
