import {
  automationActivity,
  experimentRuleIds,
  sameRuleContent,
  type RuleEnabledRequest,
} from "../automation-state.js";
import { DraftBaseline } from "../draft-state.js";
import "../draft-status/draft-status.js";
import "../rule-activation/rule-activation.js";
import initialState from "../initial-state.json";
import { attr, observable } from "@microsoft/webui-framework";
import "../script-editor/script-editor.js";
import "../match-editor/match-editor.js";
import "../user-agent-overrides/user-agent-overrides.js";
import "../response-body-editor/response-body-editor.js";
import "../response-set-workspace/response-set-workspace.js";
import "../response-set-demos/response-set-demos.js";
import type { ResponseSetDemos } from "../response-set-demos/response-set-demos.js";
import "../header-rules/header-rules.js";
import { headerRulePhase, type HeaderRules } from "../header-rules/header-rules.js";
import type { ResponseSetWorkspace } from "../response-set-workspace/response-set-workspace.js";
import type {
  ResponseSetRestoreRequest,
  ResponseSetRestored,
  ResponseSetDeleted,
} from "../response-set-lifecycle/response-set-lifecycle.js";
import type { IntakeDraftResolution } from "../response-set-intake/response-set-intake.js";
import type { ExperimentSelection, VariantEdit, TrafficRuleRequest } from "../models.js";
import {
  responseBodyLanguage,
  type BodyCommit,
  type ResponseBodyEditor,
} from "../response-body-editor/response-body-editor.js";
import type { UserAgentOverrides } from "../user-agent-overrides/user-agent-overrides.js";
import type { MatchDraft, MatchEditor } from "../match-editor/match-editor.js";
import type { ScriptEditor } from "../script-editor/script-editor.js";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { WorkspaceElement } from "../workspace-element.js";
import type {
  ResponseBodyFileInfo,
  ResponseBodyFileMode,
  MatchExample,
  MatchTestResult,
  UrlCondition,
  ResponseAssetInspection,
  ResponsePreviewIntent,
  SessionDetail,
  BodyInspection,
  AutomationCandidate,
  AutomationRule,
  AutomationStatus,
  ResponseAsset,
  AutoResponseSource,
  SelectedResponse,
} from "../models.js";
import {
  describeError,
  optionalText,
  parseHeaderLines,
  encodeHeaders,
  shortUrl,
  implicitDefaultPort,
  formatContentCodings,
  supportedContentCodings,
  editableCharacterEncoding,
  responseTextEncoding,
  clientResponseSource,
  autoResponseUnavailableReason,
} from "../utilities.js";
import { defaultWorkspace, formatBytes } from "../table-model.js";
import { ListSelection, isTextEditing } from "../list-selection.js";
import type { ComparisonSubject, ComparisonOpen } from "../comparison-models.js";
import {
  decodeBodyBase64,
  encodeBody,
  equalBodyBytes,
  renderBodyBytes,
  type BodyMode,
} from "../response-body-codec.js";

const AUTORESPONSE_PRIORITY_BASE = -1_000_000;
const MAX_AUTORESPONSE_EDIT_BYTES = 16 * 1024 * 1024;
type RuleRow = AutomationRule & {
  order: number;
  name: string;
  criteria: string;
  state: string;
  toggleLabel: string;
  first: boolean;
  last: boolean;
  selected: boolean;
  selectionState: string;
  tabIndex: string;
  shadowedName: string;
  shadowedBy: string;
  diagnostic: string;
  responseText: string;
};
type BatchRow = {
  included: boolean;
  order: number;
  id: string;
  name: string;
  method: string;
  url: string;
  eligible: boolean;
  reason: string;
  duplicate: string;
  startedAt: number;
};
type CapturedDraft = {
  sessionId: string;
  detail: SessionDetail;
  headers: ResponseAsset["headers"];
  inspection: BodyInspection | null;
};
type NativeRuleRow = {
  rule: AutomationRule;
  id: string;
  name: string;
  kind: string;
  action: string;
  direction: string;
  criteria: string;
  state: string;
  selected: boolean;
  selectionState: string;
  tabIndex: string;
};

export class AutomationWorkspace extends WorkspaceElement {
  @observable enabledScriptCount = 0;
  @observable activity = automationActivity(null, 0);
  @observable savedActivationRule: AutomationRule | null = null;
  @observable responseCanSave = false;
  private responseBaseline = new DraftBaseline();
  enabledScriptCountChanged(): void {
    this.updateActivity();
  }
  private updateActivity(): void {
    this.activity = automationActivity(this.automationStatus, this.enabledScriptCount);
  }
  dismissAutomationStatus(): void {
    this.automationText = "";
  }
  async onRuleEnabled(event: CustomEvent<RuleEnabledRequest>): Promise<void> {
    event.stopPropagation();
    await this.setRulesEnabled([event.detail], event.detail.enabled);
  }
  private async setRulesEnabled(
    requests: Array<{ id: string; revision: number }>,
    enabled: boolean,
  ): Promise<void> {
    if (this.savingAutoResponse || this.changingAutoResponse || !requests.length) {
      return;
    }
    this.changingAutoResponse = true;
    this.automationText = "Updating rules…";
    this.automationKind = "progress";
    try {
      const current = await invoke<AutomationStatus>("automation_status");
      const ids = new Set(requests.map((request) => request.id));
      for (const request of requests) {
        if (current.rules.find((rule) => rule.id === request.id)?.revision !== request.revision) {
          throw new Error(
            "A selected rule changed. Refresh the rules and try again. Your draft is preserved.",
          );
        }
      }
      const rules = current.rules.map((rule) =>
        ids.has(rule.id) && (rule.enabled !== false) !== enabled
          ? { ...rule, enabled, revision: rule.revision + 1 }
          : rule,
      );
      const changed = rules.filter((rule, index) => rule !== current.rules[index]).length;
      if (!changed) {
        this.automationText = "";
        return;
      }
      const status = await this.activateRuleSet(rules, current.generation);
      this.rememberRuleChange(
        current.rules,
        status.rules,
        `${enabled ? "Enabled" : "Disabled"} ${changed} rule${changed === 1 ? "" : "s"}.`,
        true,
      );
      this.renderAutomation(status);
      this.automationText = "";
    } catch (error) {
      this.automationText = "Rule change failed: " + describeError(error);
      this.automationKind = "error";
    } finally {
      this.changingAutoResponse = false;
    }
  }

  @observable nativeRuleRows: NativeRuleRow[] = [];
  @observable ruleDirection = "all";
  @observable catalogSelectedId = "";
  @observable nativeDetail = false;
  @observable ruleDetail = false;
  @observable hasNativeSelection = false;
  @observable experimentEditorVisible = false;
  @observable overviewVisible = false;
  @observable preferencesReady = false;
  @observable advancedRuleText = "";
  @observable catalogActivationRule: AutomationRule | null = null;
  private lastRuleSection = "responses";
  ruleKindMenu!: HTMLElement;
  newRuleButton!: HTMLButtonElement;
  focusNewRule(): void {
    this.newRuleButton.focus({ preventScroll: true });
  }
  automationSectionChanged(): void {
    if (!["experiments", "scripts"].includes(this.automationSection)) {
      this.lastRuleSection = this.automationSection;
    }
    this.updateRuleDetail();
  }
  nativeDetailChanged(): void {
    this.updateRuleDetail();
  }
  editorHiddenChanged(): void {
    this.updateRuleDetail();
  }
  batchReviewHiddenChanged(): void {
    this.updateRuleDetail();
  }
  responseExperimentChanged(): void {
    this.updateRuleDetail();
  }
  private updateRuleDetail(): void {
    this.experimentEditorVisible =
      this.responseExperiment &&
      !this.editorHidden &&
      ["responses", "experiments"].includes(this.automationSection);
    this.overviewVisible = this.view === "automation" && this.automationSection === "experiments";
    this.ruleDetail =
      this.automationSection === "responses" && this.responseExperiment
        ? false
        : ["responses", "experiments"].includes(this.automationSection)
          ? !this.editorHidden || !this.batchReviewHidden
          : this.nativeDetail;
  }
  filterRuleDirection(event: Event): void {
    this.finishRuleDrag();
    this.ruleDirection = (event.target as HTMLSelectElement).value;
    this.renderRuleSelection();
  }
  private renderNativeRules(): void {
    const status = this.automationStatus;
    if (!status) {
      return;
    }
    this.catalogActivationRule =
      status.rules.find((rule) => rule.id === this.catalogSelectedId) ?? null;
    this.advancedRuleText = this.catalogActivationRule
      ? JSON.stringify(this.catalogActivationRule, null, 2)
      : "";
    const owned = experimentRuleIds(status);
    const search = this.ruleSearch.toLowerCase();
    this.nativeRuleRows = status.rules
      .filter((rule) => !rule.request.responseAsset && !owned.has(rule.id))
      .sort((a, b) => a.priority - b.priority || a.id.localeCompare(b.id))
      .map((rule) => {
        const phase = headerRulePhase(rule),
          kind = rule.request.userAgent
            ? "headers"
            : phase
              ? "header-rules"
              : rule.response.textTransform &&
                  !rule.request.headers.length &&
                  !rule.response.headers.length
                ? "response-text"
                : "advanced";
        const direction = rule.request.userAgent || phase === "request" ? "Request" : "Response";
        const action = rule.request.userAgent
          ? "Change UA headers"
          : phase
            ? "Change " + direction.toLowerCase() + " headers"
            : rule.response.textTransform
              ? "Find and replace response text"
              : "Advanced rule";
        const url = rule.matcher.url,
          criteria = `${rule.matcher.method ?? "ANY"} ${url?.kind === "exact" ? url.value : url?.kind === "pattern" ? url.value.address : url?.kind === "regex" ? url.value.pattern : (rule.matcher.host ?? "Any host") + (rule.matcher.pathPrefix ?? "")}`;
        return {
          rule,
          id: rule.id,
          name: rule.displayName ?? action,
          kind,
          action,
          direction,
          criteria,
          state: rule.enabled === false ? "Disabled" : "Enabled",
          selected: this.ruleSelection.ids.has(rule.id),
          selectionState: String(this.ruleSelection.ids.has(rule.id)),
          tabIndex: this.ruleSelection.focused === rule.id ? "0" : "-1",
        };
      })
      .filter(
        (row) =>
          (!search ||
            (row.name + " " + row.action + " " + row.criteria).toLowerCase().includes(search)) &&
          (this.ruleDirection === "all" ||
            row.direction.toLowerCase() + "s" === this.ruleDirection) &&
          (this.ruleFilter === "all" || this.ruleFilter === row.state.toLowerCase()),
      );
  }
  private async selectNativeRule(row: NativeRuleRow): Promise<boolean> {
    const previous = this.automationSection;
    this.showAutomationSection(row.kind);
    this.$flushUpdates();
    if (row.kind === "headers") {
      await this.userAgentOverrides.select(row.id);
      if (this.userAgentOverrides.uaSelectedId !== row.id) {
        this.showAutomationSection(previous);
        return false;
      }
    } else if (row.kind === "header-rules" || row.kind === "response-text") {
      const editor = row.kind === "header-rules" ? this.headerRulesEditor : this.responseTextEditor;
      await editor.selectRule(row.id);
      if (editor.hrId !== row.id) {
        this.showAutomationSection(previous);
        return false;
      }
    }
    this.catalogSelectedId = row.id;
    this.nativeDetail = true;
    this.renderRuleSelection();
    return true;
  }
  catalogRuleClick(row: NativeRuleRow, event: MouseEvent, checkbox = false): void {
    const rule = this.automationStatus?.rules.find((rule) => rule.id === row.id);
    if (rule) {
      if (checkbox) {
        event.stopPropagation();
      }
      void this.selectRule(rule, event, checkbox);
    }
  }
  catalogRuleKey(row: NativeRuleRow, event: KeyboardEvent): void {
    const rule = this.automationStatus?.rules.find((rule) => rule.id === row.id);
    if (rule) {
      void this.ruleKeyboard(rule, event);
    }
  }
  private async openRuleProperties(rule: AutomationRule): Promise<boolean> {
    if (rule.request.responseAsset) {
      await this.editAutoResponse(rule);
      return true;
    }
    const row = this.nativeRuleRows.find((row) => row.id === rule.id);
    return row ? this.selectNativeRule(row) : false;
  }
  private showRuleSelection(): void {
    this.nativeDetail = false;
    this.editorHidden = true;
    this.automationSection = "responses";
    this.catalogSelectedId = "";
  }
  async createRule(kind: string): Promise<void> {
    this.ruleKindMenu.hidePopover();
    this.newRuleButton.focus();
    this.catalogSelectedId = "";
    if (kind === "saved") {
      await this.beginCapturedResponseFlow();
      return;
    }
    if (kind === "scratch") {
      await this.beginScratchAutoResponse();
      return;
    }
    if (kind === "browser-headers") {
      this.showAutomationSection("headers");
      await this.userAgentOverrides.add();
      this.nativeDetail = true;
      return;
    }
    const section = kind === "response-text" ? "response-text" : "header-rules";
    this.showAutomationSection(section);
    this.nativeDetail = true;
    await (section === "response-text" ? this.responseTextEditor : this.headerRulesEditor).newRule(
      undefined,
      kind === "request-headers" ? "request" : "response",
    );
  }
  responseTextEditor!: HeaderRules;
  headerRulesEditor!: HeaderRules;
  async beginRule(request: TrafficRuleRequest): Promise<void> {
    if (request.kind === "response-text") {
      await this.beginTextEdit(request.source);
      return;
    }
    if (request.kind === "browser-headers") {
      this.showAutomationSection("headers");
      await this.userAgentOverrides.addFromSource(request.source);
      return;
    }
    this.showAutomationSection("header-rules");
    await this.headerRulesEditor.newRule(
      request.source,
      request.kind === "request-headers" ? "request" : "response",
    );
  }
  async beginTextEdit(selected: SelectedResponse): Promise<void> {
    this.showAutomationSection("response-text");
    await this.responseTextEditor.newRule(selected);
  }
  compareResponseMenu!: HTMLElement;
  compareResponseButton!: HTMLButtonElement;
  previewResponseMenu!: HTMLElement;
  previewResponseButton!: HTMLButtonElement;
  private closeResponseMenu(menu: HTMLElement, button: HTMLButtonElement): void {
    if (menu?.matches(":popover-open")) {
      menu.hidePopover();
      button.focus();
    }
  }
  compareResponseDraft(mode: string): void {
    this.closeResponseMenu(this.compareResponseMenu, this.compareResponseButton);
    const data = new FormData(this.autoResponseForm),
      saved = this.savedResponse,
      source = this.autoResponseSourceState;
    try {
      const base: ComparisonSubject | null = saved
        ? { kind: "asset", reference: `${saved.asset.id}@${saved.asset.revision}` }
        : this.capturedDraft
          ? { kind: "exchange", id: this.capturedDraft.sessionId }
          : null;
      let left = base;
      if (mode === "source" && saved?.asset.provenance.kind === "session") {
        left = {
          kind: "exchange",
          id: saved.asset.provenance.exchange_id,
          responseBoundary: saved.asset.provenance.boundary,
        };
      }
      if (mode === "previous" && saved) {
        const previous = [...this.assetsIndex.values()]
          .filter((a) => a.id === saved.asset.id && a.revision < saved.asset.revision)
          .sort((a, b) => b.revision - a.revision)[0];
        if (!previous) {
          throw new Error("No earlier saved response revision is retained.");
        }
        left = { kind: "asset", reference: `${previous.id}@${previous.revision}` };
      }
      if (!left) {
        throw new Error("Choose a captured source or saved response before comparing this draft.");
      }
      const parsed = this.parseEditorHeaders("responseHeaders", data).filter(
        (h) =>
          !["content-length", "content-encoding", "content-type"].includes(h.name.toLowerCase()),
      );
      const headers = this.mergeResponseHeaders(String(data.get("responseHeaders") ?? ""), parsed);
      const media = String(data.get("mediaType") ?? "").trim();
      if (media) {
        headers.push(...encodeHeaders([{ name: "Content-Type", value: media }]));
      }
      const editable =
        source?.kind === "captured" ? this.autoResponseEditBody.checked : !this.editBodyDisabled;
      const currentBytes = !this.responseBodyPath && editable ? this.currentBodyBytes() : null;
      const decodedBody =
        currentBytes &&
        this.bodyOriginalBytes &&
        equalBodyBytes(currentBytes, this.bodyOriginalBytes)
          ? null
          : currentBytes;
      const right: ComparisonSubject = {
        kind: "draft",
        label: String(data.get("name") ?? "Response draft"),
        status: String(data.get("status") ?? ""),
        headers,
        decodedBody,
        base,
        bodyPath: this.responseBodyPath,
      };
      const comparison: ComparisonOpen = { left, right };
      this.$emit("comparison-request", comparison);
    } catch (error: unknown) {
      this.autoResponseError = describeError(error);
      this.openResponseForError();
    }
  }
  @observable responseExperiment = false;
  @observable experimentLive = false;
  @observable experimentStateText = "";
  @observable responseSaveLabel = initialState.responseSaveLabel;
  @observable responseNameLabel = initialState.responseNameLabel;
  @observable variantHistorical = false;
  responseSets!: ResponseSetWorkspace;
  responseSetDemos!: ResponseSetDemos;
  requestSavedDemos(event: CustomEvent<{ mode: "save" | "list" }>): void {
    event.stopPropagation();
    const owner = this.responseSets.current();
    if (!owner || this.responseSets.busy || this.fileChoiceBusy) {
      return;
    }
    if (event.detail.mode === "save" && this.responseSets.dirty) {
      return;
    }
    const { id, incarnation, revision } = owner;
    const current = () =>
      this.isConnected &&
      this.collectionOpen &&
      this.view === "automation" &&
      this.automationSection === "experiments" &&
      this.responseSets.setId === id &&
      this.responseSets.current()?.incarnation === incarnation &&
      this.responseSets.current()?.revision === revision;
    this.responseSetDemos.show(
      owner,
      event.detail.mode,
      current,
      !this.responseSets.dirty,
      this.selection?.sessionId ?? "",
    );
  }
  savedDemoNotice(event: CustomEvent<{ ownerId: string; demo: { name: string } }>): void {
    event.stopPropagation();
    if (this.responseSets.setId === event.detail.ownerId) {
      this.responseSets.message = `Saved demo ${event.detail.demo.name}.`;
    }
  }
  @observable previewReportAvailable = false;
  @observable previewReportSetId = "";
  @observable editorPreviewReportAvailable = false;
  previewReportAvailableChanged(): void {
    this.renderExperimentState();
  }
  previewReportSetIdChanged(): void {
    this.renderExperimentState();
  }
  showPreviewDiagnostics(): void {
    this.$emit("preview-report-request", undefined);
  }
  private editingVariant: ExperimentSelection | null = null;
  onVariantStatus(event: CustomEvent<AutomationStatus>): void {
    if (
      event.detail.generation >= (this.automationStatus?.generation ?? 0) &&
      this.automationKind === "success"
    ) {
      this.automationText = "";
    }
    this.renderAutomation(event.detail);
  }
  dispatchCollectionAdd(event: CustomEvent<{ source: string }>): void {
    if (event.detail.source === "captured" || event.detail.source === "rules") {
      this.$emit("response-set-intake-request", {
        source: event.detail.source === "rules" ? "rules" : "traffic",
        destinationId: this.responseSets.setId,
      });
      return;
    }
    void this.addCollectionResponse(event);
  }
  collectSelectedRules(): void {
    const ids = [...this.ruleSelection.ids],
      ordinary = new Set(this.rules.map((rule) => rule.id));
    this.ruleMenu.hidePopover();
    this.ruleMoreButton.focus({ preventScroll: true });
    if (!ids.length || ids.some((id) => !ordinary.has(id))) {
      this.automationText = "Select only ordinary saved-response rules to add to a response set.";
      this.automationKind = "error";
      return;
    }
    this.$emit("response-set-intake-request", { source: "rules", ids });
  }
  private intakeDraftSignature(): string {
    this.captureCollectionResponse();
    return JSON.stringify({
      selection: this.responseSets.currentSelection(),
      name: this.responseSets.setName,
      notes: this.responseSets.setNotes,
      rules: this.responseSets.draftRules,
      editor: this.editorRevision,
      snapshots: [...this.collectionSnapshots.entries()].map(([id, snapshot]) => ({
        id,
        fields: snapshot.fields,
        state: snapshot.state,
        matcher: snapshot.matcher,
      })),
    });
  }
  private intakeSourceDiscardDeferred = false;
  // Keep the editor's full immutable definition when a completed Move removes
  // its original identity while newer properties remain in the editor.
  private responseRuleTemplate: AutomationRule | null = null;
  private intakeSourceSignature(ids: string[]): string | null {
    if (
      this.collectionOpen ||
      this.responseExperiment ||
      this.editorHidden ||
      !this.editingAutoResponseId ||
      !ids.includes(this.editingAutoResponseId)
    ) {
      return null;
    }
    this.refreshResponseDirty();
    return JSON.stringify({
      id: this.editingAutoResponseId,
      editor: this.editorRevision,
      values: this.responseValues(),
      body: this.autoResponseBody.value,
      bodyMode: this.bodyMode,
      encoding: this.bodyTextEncoding,
      asset: this.autoResponseSourceState,
    });
  }
  private async prepareIntakeSources(
    ids: string[],
    current: () => boolean,
  ): Promise<IntakeDraftResolution | null> {
    if (!ids.length) {
      return { afterCommit: async () => {} };
    }
    const affected =
      !this.collectionOpen &&
      !this.responseExperiment &&
      !this.editorHidden &&
      !!this.editingAutoResponseId &&
      ids.includes(this.editingAutoResponseId);
    if (
      affected &&
      (this.loadingSavedResponse ||
        this.fileChoiceBusy ||
        this.savingAutoResponse ||
        this.changingAutoResponse ||
        this.resolvingUnsaved ||
        this.bodyEditor?.surface?.open)
    ) {
      return null;
    }
    if (this.intakeSourceSignature(ids) !== null && this.draftDirty) {
      this.intakeSourceDiscardDeferred = true;
      try {
        if (!(await this.canLeaveEditor()) || !current()) {
          return null;
        }
      } finally {
        this.intakeSourceDiscardDeferred = false;
      }
    }
    if (!current()) {
      return null;
    }
    const signature = this.intakeSourceSignature(ids);
    return {
      beforeCommit: () => signature === this.intakeSourceSignature(ids),
      afterCommit: async (committed) => {
        if (
          !this.isConnected ||
          this.collectionOpen ||
          this.responseExperiment ||
          this.editorHidden ||
          !this.editingAutoResponseId ||
          !ids.includes(this.editingAutoResponseId)
        ) {
          return;
        }
        const latest =
          this.automationStatus && this.automationStatus.generation >= committed.generation
            ? this.automationStatus
            : committed;
        if (latest.rules.some((rule) => rule.id === this.editingAutoResponseId)) {
          return;
        }
        const unchanged = signature === this.intakeSourceSignature(ids);
        if (!unchanged && this.draftDirty) {
          // The operation may finish after its dialog closes. Keep newer work as a
          // new rule draft rather than leaving Save pointed at the removed source.
          this.editingAutoResponseId = null;
          this.existingResponse = false;
          this.savedActivationRule = null;
          this.editorTitle = "New saved-response rule";
          this.draftDirty = true;
          this.responseCanSave = true;
          this.autoResponseError =
            "The original rule was moved. Your newer changes are preserved as a new rule draft.";
          this.renderExperimentState();
          return;
        }
        this.editorRevision++;
        this.editorHidden = true;
        this.editingAutoResponseId = null;
        this.autoResponseSourceState = null;
        this.savedActivationRule = null;
        this.draftDirty = false;
        this.responseCanSave = false;
      },
    };
  }
  async prepareResponseSetIntake(
    id: string,
    current: () => boolean,
    movingRuleIds: string[] = [],
  ): Promise<IntakeDraftResolution | null> {
    // Check the destination before offering to discard any source work.
    const status = await invoke<AutomationStatus>("automation_status");
    if (!current() || (id !== "__new" && !status.responseSets?.some((set) => set.id === id))) {
      return null;
    }
    const source = await this.prepareIntakeSources(movingRuleIds, current);
    if (!source) {
      return null;
    }
    const destination =
      id === "__new"
        ? { afterCommit: async () => {} }
        : await this.prepareIntakeDestination(id, current);
    if (!destination) {
      return null;
    }
    return {
      beforeCommit: () => source.beforeCommit?.() ?? true,
      afterCommit: async (committed) => {
        await source.afterCommit(committed);
        await destination.afterCommit(committed);
      },
    };
  }
  private async prepareIntakeDestination(
    id: string,
    current: () => boolean,
  ): Promise<IntakeDraftResolution | null> {
    const status = await invoke<AutomationStatus>("automation_status");
    if (!current() || !status.responseSets?.some((set) => set.id === id)) {
      return null;
    }
    if (!this.collectionOpen || this.responseSets.setId !== id) {
      return { afterCommit: async () => {} };
    }
    if (!(await this.canLeaveEditor()) || !current() || this.responseSets.setId !== id) {
      return null;
    }
    const discard = this.responseSets.dirty,
      incarnation = this.responseSets.currentSelection().incarnation,
      signature = this.intakeDraftSignature();
    return {
      afterCommit: async (committed) => {
        if (
          !this.isConnected ||
          !this.collectionOpen ||
          this.responseSets.setId !== id ||
          signature !== this.intakeDraftSignature()
        ) {
          return;
        }
        const saved = committed.responseSets?.find(
          (set) => set.id === id && set.incarnation === incarnation,
        );
        if (!saved) {
          return;
        }
        const latest =
          this.automationStatus && this.automationStatus.generation >= committed.generation
            ? this.automationStatus
            : committed;
        const set = latest.responseSets?.find(
          (set) => set.id === id && set.incarnation === incarnation,
        );
        if (!set || set.revision !== saved.revision) {
          return;
        }
        const selected = this.collectionRuleId,
          rule = set.rules.find((rule) => rule.id === selected),
          snapshot = this.collectionSnapshots.get(selected),
          resetEditor = discard && snapshot?.state.draftDirty === true;
        // Intake only appends definitions. Clean editor snapshots still describe the
        // same immutable assets; discarded drafts must not survive the new baseline.
        this.collectionSnapshots = new Map(
          [...this.collectionSnapshots].filter(
            ([ruleId, snapshot]) =>
              set.rules.some((rule) => rule.id === ruleId) &&
              (!discard || snapshot.state.draftDirty !== true),
          ),
        );
        this.responseSets.saved(latest);
        this.collectionName = set.name;
        this.editingVariant = this.responseSets.currentSelection();
        if (this.responseSets.nameInput.value !== set.name) {
          this.responseSets.nameInput.value = set.name;
        }
        if (this.responseSets.notesInput.value !== set.notes) {
          this.responseSets.notesInput.value = set.notes;
        }
        this.responseSets.select(rule ? selected : "");
        if (!rule) {
          this.collectionRuleId = "";
          this.editorRevision++;
          this.editorHidden = true;
          this.draftDirty = false;
          return;
        }
        if (!resetEditor) {
          return;
        }
        const root = this.getRootNode(),
          focus = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
        let restore = focus instanceof HTMLElement && this.autoResponseForm.contains(focus);
        const moved = (event: Event) => {
          if (event.composedPath()[0] !== focus) {
            restore = false;
          }
        };
        root.addEventListener("focusin", moved);
        root.addEventListener("pointerdown", moved);
        this.switchingCollection = true;
        try {
          await this.editAutoResponse(rule, true, true, this.responseSets.currentSelection());
        } finally {
          this.switchingCollection = false;
          root.removeEventListener("focusin", moved);
          root.removeEventListener("pointerdown", moved);
          if (
            restore &&
            focus instanceof HTMLElement &&
            focus.isConnected &&
            focus.getClientRects().length
          ) {
            focus.focus({ preventScroll: true });
          }
        }
      },
    };
  }
  onHeaderRulesState(event: CustomEvent<AutomationStatus>): void {
    const editor =
      this.automationSection === "header-rules"
        ? this.headerRulesEditor
        : this.automationSection === "response-text"
          ? this.responseTextEditor
          : null;
    if (editor) {
      this.catalogSelectedId = editor.hrId;
    }
    this.renderAutomation(event.detail);
    const section = this.automationSection;
    queueMicrotask(() => {
      if (editor && this.automationSection === section) {
        this.catalogSelectedId = editor.hrId;
        if (editor.hrId) {
          this.ruleSelection.replace(editor.hrId);
        }
        this.renderRuleSelection();
      }
    });
  }
  private collectionSnapshots = new Map<
    string,
    {
      fields: Array<{ name: string; value: string; checked: boolean; customError: string }>;
      state: Record<string, unknown>;
      baseline: DraftBaseline;
      matcher: MatchDraft;
    }
  >();
  private collectionRuleId = "";
  private switchingCollection = false;
  private stagingCollection = false;
  private stagedRule: AutomationRule | null = null;
  private collectionAddEpoch = 0;
  @observable collectionAdding = false;
  @observable collectionOpen = false;
  @observable collectionName = "";
  private readonly collectionStateKeys = [
    "editingAutoResponseId",
    "autoResponseSourceState",
    "responseRuleTemplate",
    "savedResponse",
    "responseBodyPath",
    "responseBodyFileMode",
    "responseBodyFile",
    "responseBodyFileDetails",
    "responseBodyFileError",
    "responseCopiedFrom",
    "responseBodyFileBytes",
    "bodyFileTouched",
    "savedHeaderText",
    "originalResponseHeaders",
    "capturedDraft",
    "capturedOrigin",
    "existingResponse",
    "sourceText",
    "sourceSessionId",
    "sourceLabel",
    "sourceAvailable",
    "sourceAvailabilityText",
    "bodyStatusText",
    "bodyMode",
    "bodyTextEncoding",
    "bodyOriginalBytes",
    "bodyViewBytes",
    "bodyViewValue",
    "bodyByteSummary",
    "bodyHidden",
    "encodingOptionsHidden",
    "preserveEncodingDisabled",
    "responseReadOnly",
    "responseHeadersHidden",
    "capturedOptionsHidden",
    "bodyReadOnly",
    "requestHeadersHidden",
    "editBodyDisabled",
    "responseFieldsHidden",
    "retainAdvancedMatcher",
    "advancedMatcherText",
    "matchExamples",
    "matcherCondition",
    "requestHeadersOpen",
    "customMethodHidden",
    "responseOpen",
    "editorTitle",
    "autoResponseError",
    "requestHeadersError",
    "responseHeadersError",
    "draftDirty",
    "savedNonBodySnapshot",
  ] as const;
  private captureCollectionResponse(): void {
    if (!this.collectionRuleId || this.editorHidden || this.loadingSavedResponse) {
      return;
    }
    this.requestHeadersOpen =
      (this.autoResponseForm.elements.namedItem("requestHeaders") as HTMLTextAreaElement).closest(
        "details",
      )?.open ?? false;
    this.refreshResponseDirty();
    const fields = Array.from(this.autoResponseForm.elements)
      .filter(
        (field): field is HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement =>
          field instanceof HTMLInputElement ||
          field instanceof HTMLTextAreaElement ||
          field instanceof HTMLSelectElement,
      )
      .filter((field) => !!field.name)
      .map((field) => ({
        name: field.name,
        value: field.value,
        checked: field instanceof HTMLInputElement && field.checked,
        customError: field.validity.customError ? field.validationMessage : "",
      }));
    this.collectionSnapshots.set(this.collectionRuleId, {
      fields,
      state: Object.fromEntries(
        this.collectionStateKeys.map((key) => [key, Reflect.get(this, key)]),
      ),
      baseline: this.responseBaseline,
      matcher: this.matchEditor.captureDraft(),
    });
    this.responseSets.markEditorDirty(
      [...this.collectionSnapshots.values()].some((snapshot) => snapshot.state.draftDirty === true),
    );
  }
  private restoreCollectionResponse(id: string): boolean {
    const snapshot = this.collectionSnapshots.get(id);
    if (!snapshot) {
      return false;
    }
    this.editorRevision++;
    this.fileInfoRevision++;
    window.clearTimeout(this.matcherTimer);
    this.matcherTestRevision++;
    for (const [key, value] of Object.entries(snapshot.state)) {
      Reflect.set(this, key, value);
    }
    this.responseBaseline = snapshot.baseline;
    this.editingVariant = this.responseSets.currentSelection();
    this.responseExperiment = true;
    this.editorHidden = false;
    this.loadingSavedResponse = false;
    this.$flushUpdates();
    for (const field of snapshot.fields) {
      const element = this.autoResponseForm.elements.namedItem(field.name);
      if (
        element instanceof HTMLInputElement ||
        element instanceof HTMLTextAreaElement ||
        element instanceof HTMLSelectElement
      ) {
        element.value = field.value;
        element.setCustomValidity(field.customError);
        if (element instanceof HTMLInputElement) {
          element.checked = field.checked;
        }
      }
    }
    this.matchEditor.condition = this.matcherCondition;
    this.matchEditor.$flushUpdates();
    this.matchEditor.restoreDraft(snapshot.matcher);
    this.updateBodySummary();
    this.renderExperimentState();
    return true;
  }
  async openResponseSet(
    event: CustomEvent<{ id: string; source?: SelectedResponse | null }>,
    confirmed = false,
    focus = true,
  ): Promise<void> {
    const id = event.detail.id;
    if (!this.automationStatus?.responseSets?.some((set) => set.id === id)) {
      return;
    }
    if (this.resolvingUnsaved || this.responseSets.busy) {
      return;
    }
    if (!confirmed && !(await this.canLeaveEditor())) {
      return;
    }
    this.collectionSnapshots.clear();
    this.collectionRuleId = "";
    this.editorRevision++;
    this.editorHidden = true;
    this.draftDirty = false;
    this.responseSets.open(id);
    this.collectionOpen = true;
    this.collectionName = this.responseSets.setName;
    this.automationSection = "experiments";
    this.$flushUpdates();
    if (focus) {
      this.responseSets.backButton.focus({ preventScroll: true });
    }
    if (event.detail.source) {
      await this.addCollectionResponse(
        new CustomEvent("set-add", {
          detail: { source: "captured", captured: event.detail.source },
        }),
      );
    }
  }
  async leaveResponseSet(): Promise<void> {
    if (!(await this.canLeaveEditor())) {
      return;
    }
    this.collectionSnapshots.clear();
    this.collectionRuleId = "";
    this.editorRevision++;
    this.editorHidden = true;
    this.draftDirty = false;
    this.collectionOpen = false;
    this.responseSets.close();
  }
  async editCollectionResponse(event: CustomEvent<{ ruleId: string }>): Promise<void> {
    if (
      this.collectionAdding ||
      this.responseSets.busy ||
      this.loadingSavedResponse ||
      this.fileChoiceBusy
    ) {
      return;
    }
    const id = event.detail.ruleId,
      rule = this.responseSets.draftRules.find((rule) => rule.id === id);
    if (!rule || (id === this.collectionRuleId && !this.editorHidden)) {
      return;
    }
    this.captureCollectionResponse();
    this.collectionRuleId = id;
    this.responseSets.select(id);
    this.switchingCollection = true;
    try {
      if (!this.restoreCollectionResponse(id)) {
        await this.editAutoResponse(rule, true, true, this.responseSets.currentSelection());
      }
    } finally {
      this.switchingCollection = false;
    }
  }
  async addCollectionResponse(
    event: CustomEvent<{ source: string; captured?: SelectedResponse }>,
  ): Promise<void> {
    if (
      this.collectionAdding ||
      this.responseSets.busy ||
      this.loadingSavedResponse ||
      this.fileChoiceBusy
    ) {
      return;
    }
    if (this.responseSets.draftRules.length >= 1000) {
      this.responseSets.error = "A response set can contain at most 1,000 responses.";
      return;
    }
    const source = event.detail.source,
      captured = event.detail.captured ?? this.selection;
    if (source === "captured" && !captured?.reusable) {
      this.responseSets.error =
        "Select a reusable response in Traffic, then return here to add it.";
      return;
    }
    this.captureCollectionResponse();
    this.collectionRuleId = "";
    this.responseSets.select("");
    this.switchingCollection = true;
    this.collectionAdding = true;
    const epoch = ++this.collectionAddEpoch,
      selection = JSON.stringify(this.responseSets.currentSelection());
    try {
      let revision: number;
      if (source === "captured" && captured) {
        const pending = this.populateCapturedAutoResponse(
          captured.sessionId,
          captured.detail,
          undefined,
          true,
          this.responseSets.currentSelection(),
          true,
        );
        revision = this.editorRevision;
        await pending;
      } else {
        const pending = this.beginScratchAutoResponse(true);
        revision = this.editorRevision;
        await pending;
        this.responseExperiment = true;
        this.automationSection = "experiments";
        this.editingVariant = this.responseSets.currentSelection();
        this.renderExperimentState();
      }
      if (
        epoch !== this.collectionAddEpoch ||
        revision !== this.editorRevision ||
        !this.isConnected ||
        !this.collectionOpen ||
        selection !== JSON.stringify(this.responseSets.currentSelection())
      ) {
        return;
      }
      if (source === "captured" && this.autoResponseSourceState?.kind !== "captured") {
        return;
      }
      this.collectionRuleId = "autoresponse-rule-" + crypto.randomUUID().replaceAll("-", "");
      this.draftDirty = true;
      const data = new FormData(this.autoResponseForm),
        rule: AutomationRule = {
          id: this.collectionRuleId,
          displayName: String(data.get("name") ?? "New response") || "New response",
          enabled: true,
          revision: 0,
          priority: 0,
          matcher: {
            method: "GET",
            url: { kind: "exact", value: "" },
            requestHeaders: [],
            responseHeaders: [],
            scheme: null,
            host: null,
            port: null,
            pathPrefix: null,
            query: null,
            responseStatus: null,
            responseStatusClass: null,
          },
          request: {
            headers: [],
            replaceBody: null,
            discardBody: false,
            abortReason: null,
            responseAsset: null,
            allowNonIdempotentBodyReplacement: false,
          },
          response: { headers: [], replaceBody: null, discardBody: false, abortReason: null },
        };
      this.responseSets.stageRule(rule);
      this.responseSets.select(rule.id);
      this.captureCollectionResponse();
      if (source === "file") {
        await this.chooseResponseBody();
      }
    } finally {
      if (epoch === this.collectionAddEpoch) {
        this.switchingCollection = false;
        this.collectionAdding = false;
      }
    }
  }
  removeCollectionResponse(event: CustomEvent<{ ruleId: string }>): void {
    if (
      this.collectionAdding ||
      this.responseSets.busy ||
      this.loadingSavedResponse ||
      this.fileChoiceBusy
    ) {
      return;
    }
    const id = event.detail.ruleId;
    this.collectionSnapshots.delete(id);
    this.responseSets.removeRule(id);
    if (id === this.collectionRuleId) {
      this.editorRevision++;
      this.editorHidden = true;
      this.collectionRuleId = "";
      this.draftDirty = false;
    }
    this.responseSets.markEditorDirty(
      [...this.collectionSnapshots.values()].some((snapshot) => snapshot.state.draftDirty === true),
    );
    this.$flushUpdates();
    this.responseSets.addButton.focus({ preventScroll: true });
  }
  async discardCollection(): Promise<void> {
    if (
      this.collectionAdding ||
      this.responseSets.busy ||
      this.loadingSavedResponse ||
      this.fileChoiceBusy
    ) {
      return;
    }
    const id = this.responseSets.setId,
      selected = this.collectionRuleId;
    this.collectionSnapshots.clear();
    this.editorRevision++;
    this.collectionRuleId = "";
    this.editorHidden = true;
    this.draftDirty = false;
    this.responseSets.open(id);
    if (this.responseSets.draftRules.some((rule) => rule.id === selected)) {
      await this.editCollectionResponse(
        new CustomEvent("set-edit", { detail: { ruleId: selected } }),
      );
    }
  }
  async saveCollection(): Promise<boolean> {
    if (
      this.collectionAdding ||
      this.responseSets.busy ||
      this.responseSets.tourStep > 0 ||
      this.loadingSavedResponse ||
      this.fileChoiceBusy ||
      !this.collectionOpen
    ) {
      return false;
    }
    this.captureCollectionResponse();
    if (!this.responseSets.validate()) {
      return false;
    }
    const selected = this.collectionRuleId,
      selection = this.responseSets.currentSelection(),
      name = this.responseSets.setName.trim(),
      notes = this.responseSets.setNotes,
      focus = (this.getRootNode() as ShadowRoot).activeElement;
    if (
      !Number.isSafeInteger(selection.revision) ||
      !Number.isSafeInteger(this.automationStatus?.generation)
    ) {
      this.responseSets.error = "Refresh the response set before saving.";
      return false;
    }
    this.responseSets.busy = true;
    this.responseSets.error = "";
    this.stagingCollection = true;
    const staged: AutomationRule[] = [];
    try {
      for (const original of this.responseSets.draftRules) {
        const snapshot = this.collectionSnapshots.get(original.id);
        if (snapshot && (snapshot.state.draftDirty === true || !original.request.responseAsset)) {
          this.collectionRuleId = original.id;
          this.restoreCollectionResponse(original.id);
          this.draftDirty = true;
          this.stagedRule = null;
          if (!this.autoResponseForm.checkValidity()) {
            this.autoResponseError = "Review the required response fields.";
            throw new Error(`Review “${original.displayName ?? "New response"}”.`);
          }
          if (
            !(await this.saveAutoResponse(new Event("submit", { cancelable: true }))) ||
            !this.stagedRule
          ) {
            throw new Error(this.autoResponseError || "Response changes could not be prepared.");
          }
          staged.push({
            ...(this.stagedRule as AutomationRule),
            enabled: original.enabled !== false,
          });
        } else {
          staged.push(original);
        }
      }
      const current = await invoke<AutomationStatus>("automation_status"),
        saved = current.responseSets?.find((set) => set.id === selection.experimentId);
      if (
        !saved ||
        saved.incarnation !== selection.incarnation ||
        saved.revision !== selection.revision
      ) {
        throw new Error(
          "This response set changed. Your draft is preserved. Reopen the set after saving your work elsewhere.",
        );
      }
      const retained = [
        ...saved.rules,
        ...saved.versions.flatMap((version) => version.rules),
        ...(saved.applied?.rules ?? []),
        ...(saved.undoApply?.applied?.rules ?? []),
      ];
      const rules = staged.map((rule, index) => {
        const priority = AUTORESPONSE_PRIORITY_BASE + index,
          previous = saved.rules.find((item) => item.id === rule.id),
          maximum = Math.max(
            0,
            ...retained.filter((item) => item.id === rule.id).map((item) => item.revision),
          );
        const changed =
          JSON.stringify({ ...rule, revision: 0, priority }) !==
          JSON.stringify(previous ? { ...previous, revision: 0 } : null);
        return {
          ...rule,
          priority,
          revision: changed
            ? Math.max(rule.revision, maximum) + 1
            : (previous?.revision ?? rule.revision),
        };
      });
      const status = await invoke<AutomationStatus>("change_response_set", {
        generation: current.generation,
        change: {
          action: "save-current",
          id: selection.experimentId,
          expectedRevision: selection.revision,
          name,
          notes,
          rules,
        },
      });
      this.renderAutomation(status);
      this.responseSets.saved(status);
      this.collectionName = name;
      this.collectionSnapshots.clear();
      this.collectionRuleId = "";
      this.editorHidden = true;
      this.draftDirty = false;
      this.responseSets.busy = false;
      this.stagingCollection = false;
      this.switchingCollection = true;
      if (selected && rules.some((rule) => rule.id === selected)) {
        this.collectionRuleId = selected;
        this.responseSets.select(selected);
        await this.editAutoResponse(
          rules.find((rule) => rule.id === selected)!,
          true,
          true,
          this.responseSets.currentSelection(),
        );
      }
      if (
        focus instanceof HTMLElement &&
        focus.isConnected &&
        ((this.getRootNode() as ShadowRoot).activeElement === focus ||
          (this.getRootNode() as ShadowRoot).activeElement === null)
      ) {
        focus.focus({ preventScroll: true });
      }
      return true;
    } catch (error) {
      this.responseSets.error = describeError(error);
      if (this.collectionRuleId) {
        this.responseSets.select(this.collectionRuleId);
      }
      return false;
    } finally {
      this.stagingCollection = false;
      this.switchingCollection = false;
      this.responseSets.busy = false;
      this.savingAutoResponse = false;
    }
  }
  previewVariant(event: CustomEvent<VariantEdit>): void {
    const { selection, ruleId } = event.detail,
      set = this.automationStatus?.responseSets?.find((set) => set.id === selection.experimentId),
      sourceRule = set?.rules.find((rule) => rule.id === ruleId) ?? set?.rules[0];
    const asset = [...this.assetsIndex.values()].find(
        (asset) => `${asset.id}@${asset.revision}` === sourceRule?.request.responseAsset,
      ),
      sourceSessionId = asset?.provenance.kind === "session" ? asset.provenance.exchange_id : "",
      current = this.responseSets.previewContext();
    this.$emit("page-preview-request", {
      ruleId,
      sourceSessionId,
      mode: "modified",
      variant: selection,
      responseSetId: selection.experimentId,
      isCurrent: () => this.view === "automation" && current(),
    } satisfies ResponsePreviewIntent);
  }
  private responseSetDraftToken(): string {
    this.captureCollectionResponse();
    return JSON.stringify({
      selection: this.responseSets.currentSelection(),
      name: this.responseSets.setName,
      notes: this.responseSets.setNotes,
      rules: this.responseSets.draftRules,
      drafts: [...this.collectionSnapshots.entries()].map(([id, snapshot]) => ({
        id,
        fields: snapshot.fields,
        matcher: snapshot.matcher,
      })),
    });
  }
  async requestResponseSetRestore(event: CustomEvent<ResponseSetRestoreRequest>): Promise<void> {
    event.stopPropagation();
    const request = event.detail,
      current = () =>
        this.collectionOpen &&
        this.view === "automation" &&
        this.automationSection === "experiments" &&
        this.responseSets.setId === request.id &&
        this.responseSets.currentSelection().incarnation === request.incarnation;
    const set = this.automationStatus?.responseSets?.find(
      (set) => set.id === request.id && set.incarnation === request.incarnation,
    );
    if (
      !current() ||
      !set?.versions.some(
        (version) =>
          version.id === request.versionId && version.revision === request.versionRevision,
      )
    ) {
      request.resolve(false);
      return;
    }
    if (!(await this.canLeaveEditor()) || !current()) {
      request.resolve(false);
      return;
    }
    const token = this.responseSetDraftToken();
    request.resolve(true, () => current() && token === this.responseSetDraftToken());
  }
  restoredResponseSet(event: CustomEvent<ResponseSetRestored>): void {
    event.stopPropagation();
    const { id, incarnation } = event.detail;
    if (
      !this.collectionOpen ||
      this.responseSets.setId !== id ||
      this.responseSets.currentSelection().incarnation !== incarnation
    ) {
      return;
    }
    void this.openResponseSet(new CustomEvent("set-open", { detail: { id } }), true, false);
  }
  deletedResponseSet(event: CustomEvent<ResponseSetDeleted>): void {
    event.stopPropagation();
    const { id, incarnation, focus } = event.detail;
    if (
      !this.collectionOpen ||
      this.responseSets.setId !== id ||
      this.responseSets.currentSelection().incarnation !== incarnation
    ) {
      return;
    }
    this.collectionSnapshots.clear();
    this.collectionRuleId = "";
    this.editorRevision++;
    this.editorHidden = true;
    this.draftDirty = false;
    this.collectionOpen = false;
    this.responseSets.close(
      focus && this.view === "automation" && this.automationSection === "experiments",
    );
  }
  private renderExperimentState(): void {
    this.editorPreviewReportAvailable =
      this.previewReportAvailable && this.responseSets?.setId === this.previewReportSetId;
    this.responseSaveLabel = this.responseExperiment ? "Save changes" : "Save rule";
    this.responseNameLabel = this.responseExperiment ? "Response name" : "Rule name";
    this.variantHistorical = false;
    this.experimentStateText = this.responseExperiment
      ? "Response changes are saved together with this collection."
      : "";
  }
  userAgentOverrides!: UserAgentOverrides;
  @observable headerOverrideCount = 0;
  scriptEditor!: ScriptEditor;
  @attr view = "traffic";
  @attr theme = "system";
  @observable automationSection = "responses";
  @observable batchActionId = "";
  async showAutomationSection(section: string): Promise<void> {
    if (
      this.collectionOpen &&
      this.automationSection === "experiments" &&
      section !== "experiments"
    ) {
      if (!(await this.canLeaveEditor())) {
        return;
      }
      this.collectionSnapshots.clear();
      this.collectionRuleId = "";
      this.collectionOpen = false;
      this.editorRevision++;
      this.editorHidden = true;
      this.responseExperiment = false;
      this.draftDirty = false;
      this.responseSets.close(false);
    }
    this.finishRuleDrag();
    this.automationSection = section === "rules" ? this.lastRuleSection : section;
    this.nativeDetail = !["responses", "experiments", "scripts"].includes(this.automationSection);
  }
  async backToRules(): Promise<void> {
    if (this.nativeDetail) {
      this.nativeDetail = false;
      this.$flushUpdates();
      this.newRuleButton.focus();
      return;
    }
    if (!this.batchReviewHidden) {
      this.cancelBatch();
      return;
    }
    if (!(await this.canLeaveEditor())) {
      return;
    }
    const focused = this.ruleSelection.focused;
    this.editorRevision++;
    this.editorHidden = true;
    this.autoResponseSourceState = null;
    this.editingAutoResponseId = null;
    this.draftDirty = false;
    this.$flushUpdates();
    if (focused) {
      this.focusRule(focused);
    } else {
      this.newRuleButton.focus();
    }
  }
  openBatchMenu(id: string, event: MouseEvent): void {
    this.batchActionId = id;
    this.positionRuleMenu(event);
  }
  includeBatchRow(id: string, event: Event): void {
    const included = (event.currentTarget as HTMLInputElement).checked;
    this.batchRows = this.batchRows.map((row) => (row.id === id ? { ...row, included } : row));
    this.renderBatchReview();
  }
  batchKeyboard(id: string, event: KeyboardEvent): void {
    if (isTextEditing(event)) {
      return;
    }
    if (event.key === "Delete") {
      event.preventDefault();
      this.skipBatchRow(id);
    } else if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
      event.preventDefault();
      this.moveBatchRow(id, event.key === "ArrowUp" ? -1 : 1);
    }
  }
  @observable selection: SelectedResponse | null = initialState.selection;
  @observable autoresponseState: AutomationStatus | null = null;
  @observable autoresponsePending = false;
  @observable matcherCondition: UrlCondition | null = null;
  @observable matchTestResult: MatchTestResult | null = null;
  @observable matcherTestError = "";
  @observable matcherTestBusy = false;
  @observable matchExamples: MatchExample[] = [];
  @observable matchExpected = "match";
  @observable matchCaseRows: Array<{
    index: number;
    method: string;
    url: string;
    headers: string;
    expected: string;
    actual: string;
    result: string;
    passed: boolean;
    checked: boolean;
  }> = [];
  private matchExpectationChosen = false;
  matchExamplesChanged(): void {
    this.renderMatchCases();
  }
  private renderMatchCases(result?: MatchTestResult): void {
    this.matchCaseRows = this.matchExamples.map((example, index) => {
      const actual = result?.examples.find((item) => item.index === index);
      return {
        index,
        method: example.method,
        url: example.url,
        headers: (example.headers ?? [])
          .map((header) => header.name + ": " + header.value)
          .join("\n"),
        expected: example.expected ? "Matches" : "Does not match",
        actual: actual ? (actual.matched ? "Matches" : "Does not match") : "Not checked",
        result: actual ? (actual.passed ? "Passed" : "Failed") : "Not checked",
        passed: actual?.passed ?? false,
        checked: actual !== undefined,
      };
    });
  }
  testRequestChanged(): void {
    this.matchExpectationChosen = false;
    this.scheduleMatcherTest();
  }
  changeMatchExpectation(event: Event): void {
    this.matchExpected = (event.currentTarget as HTMLSelectElement).value;
    this.matchExpectationChosen = true;
  }
  @observable responseBodyFile = "";
  @observable responseBodyFileDetails = "";
  @observable responseBodyFileError = "";
  @observable responseBodyFileMode: ResponseBodyFileMode = "copy";
  @observable responseCopiedFrom = "";
  private responseBodyFileBytes: number | null = null;
  private fileInfoRevision = 0;
  @observable fileChoiceBusy = false;
  @observable compressionType = "";
  @observable compressionUnavailableText = "";
  private bodyFileTouched = false;
  @observable existingResponse = false;
  @observable ruleUsageText = "";
  @observable requestHeadersOpen = false;
  @observable responseOpen = true;
  responseToggled(event: Event): void {
    this.responseOpen = (event.currentTarget as HTMLDetailsElement).open;
  }
  openResponseForError(): void {
    this.responseOpen = true;
    this.$flushUpdates();
  }
  @observable customMethodHidden = true;
  @observable advancedMatcherText = "";
  autoResponseCustomMethod!: HTMLInputElement;
  private retainAdvancedMatcher = true;
  matchEditor!: MatchEditor;
  matchTestUrl!: HTMLInputElement;
  matchTestHeaders!: HTMLTextAreaElement;
  matchTestMethod!: HTMLInputElement;
  autoResponseEnabled!: HTMLInputElement;
  private savedResponse: ResponseAssetInspection | null = null;
  private responseBodyPath: string | null = null;
  private matcherTimer: number | undefined;
  private usageTimer: number | undefined;
  private matcherTestRevision = 0;
  private savedHeaderText = "";
  private originalResponseHeaders: Array<{
    field: ResponseAsset["headers"][number];
    line: string | null;
  }> = [];
  private capturedDraft: CapturedDraft | null = null;
  private capturedOrigin: { sessionId: string; detail: SessionDetail } | null = null;
  autoresponseStateChanged(): void {
    if (this.autoresponseState && this.autoresponseState !== this.automationStatus) {
      this.renderAutomation(this.autoresponseState);
    }
  }
  @observable automationText = initialState.automationText;
  @observable automationKind = initialState.automationKind;
  @observable rules: RuleRow[] = initialState.rules;
  @observable visibleRules: RuleRow[] = [];
  @observable ruleSearch = "";
  @observable ruleFilter = "all";
  @observable ruleSelectionCount = 0;
  @observable ruleSelectionText = "Select a rule to edit its properties.";
  @observable ruleDropId = "";
  @observable ruleDropAfter = false;
  @observable ruleDragging = false;
  @observable ruleDragText = "";
  @observable ruleDragLabel = "";
  @observable ruleDragX = "0px";
  @observable ruleDragY = "0px";
  @observable ruleDiagnosticText = "";
  @observable ruleSupersededBy = "";
  @observable draftDirty = false;
  @observable batchReviewHidden = true;
  @observable batchRows: BatchRow[] = [];
  @observable batchLoading = false;
  @observable batchEligibleCount = 0;
  @observable batchReviewText = "";
  @observable batchError = "";
  @observable ruleListShare = "45%";
  @observable ruleListSplit = 45;
  @observable ruleMenuX = "12px";
  @observable ruleMenuY = "80px";
  @observable loadingSavedResponse = false;
  ruleMoreButton!: HTMLButtonElement;
  @observable preferences = defaultWorkspace();
  @observable allRulesSelected = false;
  ruleMenu!: HTMLElement;
  unsavedDialog!: HTMLDialogElement;
  private ruleSelection = new ListSelection();
  private assetsIndex = new Map<string, ResponseAsset>();
  private ruleHistory: Array<{
    before: AutomationRule[];
    after: AutomationRule[];
    label: string;
    all?: boolean;
  }> = [];
  private leaveEditorPromise: Promise<boolean> | null = null;
  private resolveLeave: ((value: boolean) => void) | null = null;
  private batchGeneration = 0;
  @observable editorHidden = initialState.editorHidden;
  @observable editorTitle = initialState.editorTitle;
  @observable sourceText = initialState.sourceText;
  @observable sourceSessionId = initialState.sourceSessionId;
  @observable sourceLabel = initialState.sourceLabel;
  @observable sourceAvailable = initialState.sourceAvailable;
  @observable sourceAvailabilityText = initialState.sourceAvailabilityText;
  @observable bodyStatusText = initialState.bodyStatusText;
  @observable bodyMode: BodyMode = "text";
  @observable bodyByteSummary = "";
  private bodyTextEncoding: string | null = "utf-8";
  private bodyOriginalBytes: number[] | null = null;
  private bodyViewBytes: number[] | null = null;
  private bodyViewValue = "";
  @observable bodyHidden = initialState.bodyHidden;
  @observable encodingOptionsHidden = initialState.encodingOptionsHidden;
  @observable savingAutoResponse = initialState.savingAutoResponse;
  @observable changingAutoResponse = initialState.changingAutoResponse;
  @observable autoResponseError = initialState.autoResponseError;
  @observable requestHeadersError = initialState.requestHeadersError;
  @observable responseHeadersError = initialState.responseHeadersError;
  @observable undoRemoveText = initialState.undoRemoveText;
  @observable responseReadOnly = initialState.responseReadOnly;
  @observable responseHeadersHidden = initialState.responseHeadersHidden;
  @observable capturedOptionsHidden = initialState.capturedOptionsHidden;
  @observable preserveEncodingDisabled = initialState.preserveEncodingDisabled;
  @observable bodyReadOnly = initialState.bodyReadOnly;
  @observable requestHeadersHidden = initialState.requestHeadersHidden;
  @observable editBodyDisabled = initialState.editBodyDisabled;
  @observable responseFieldsHidden = initialState.responseFieldsHidden;
  @observable highlightedRuleId: string | null = initialState.highlightedRuleId;
  autoResponseForm!: HTMLFormElement;
  autoResponseList!: HTMLDivElement;
  autoResponseMethod!: HTMLSelectElement;
  autoResponseStatus!: HTMLInputElement;
  autoResponseMediaType!: HTMLInputElement;
  autoResponseBody!: HTMLTextAreaElement;
  autoResponseEditBody!: HTMLInputElement;
  autoResponsePreserveEncoding!: HTMLInputElement;
  bodyEditor!: ResponseBodyEditor;
  bodyDialogButton!: HTMLButtonElement;
  private savedNonBodySnapshot = "";
  private bodyDialogBefore: {
    revision: number;
    value: string;
    mode: BodyMode;
    viewValue: string;
    viewBytes: number[] | null;
    status: string;
    edit: boolean;
    readOnly: boolean;
    dirty: boolean;
    error: string;
  } | null = null;
  private bodyDialogCommitted = false;
  undoRemoveButton!: HTMLButtonElement;
  private automationStatus: AutomationStatus | null = null;
  private autoResponseSourceState: AutoResponseSource | null = null;
  private editingAutoResponseId: string | null = null;
  private draggedRuleId: string | null = null;
  private draggedRuleIds: string[] = [];
  private dragGeneration: number | null = null;
  private dragOriginalSelection: ListSelection | null = null;
  private dragPointer: { x: number; y: number } | null = null;
  private dragScrollTimer: number | undefined;
  private rulePointer: { id: string; event: PointerEvent; x: number; y: number } | null = null;
  private ignoreRuleClick = false;
  private cancelRuleDrag = () => this.finishRuleDrag();
  private editorRevision = 0;
  private editorOpener: HTMLElement | null = null;
  private sourceQuery: Promise<void> | null = null;

  protected hydratedCallback(): void {
    window.addEventListener("blur", this.cancelRuleDrag);
    const generation = ++this.mcpEventGeneration;
    void listen("mcp-automation-changed", () => {
      this.refreshUsageSoon();
      if (this.view === "automation") {
        this.automationText =
          "An agent changed saved rules. Your editor drafts are preserved; review the current rule revision before saving.";
      }
    })
      .then((unlisten) => {
        if (this.isConnected && generation === this.mcpEventGeneration) {
          this.mcpUnlisten = unlisten;
        } else {
          unlisten();
        }
      })
      .catch(() => undefined);
    void this.refreshAutomation();
  }
  private mcpUnlisten: UnlistenFn | undefined;
  private mcpEventGeneration = 0;
  disconnectedCallback(): void {
    this.mcpEventGeneration++;
    this.mcpUnlisten?.();
    this.mcpUnlisten = undefined;
    this.collectionAddEpoch++;
    this.finishRuleDrag();
    window.removeEventListener("blur", this.cancelRuleDrag);
    window.clearTimeout(this.matcherTimer);
    window.clearTimeout(this.usageTimer);
    this.matcherTestRevision++;
    this.editorRevision++;
    this.resolveLeave?.(false);
    this.resolveLeave = null;
    this.leaveEditorPromise = null;
    super.disconnectedCallback();
  }
  viewChanged(): void {
    this.updateRuleDetail();
    if (this.view === "automation") {
      void this.refreshSourceAvailability();
      this.refreshUsageSoon();
    } else {
      this.finishRuleDrag();
    }
  }
  refreshUsageSoon(): void {
    if (this.usageTimer !== undefined) {
      return;
    }
    this.usageTimer = window.setTimeout(() => {
      this.usageTimer = undefined;
      if (this.view === "automation") {
        void invoke<AutomationStatus>("automation_status")
          .then((status) => {
            if (this.isConnected) {
              this.renderAutomation(status);
              void this.refreshResponseFile();
            }
          })
          .catch(() => undefined);
      }
    }, 500);
  }
  private currentMatcher(data: FormData, existing?: AutomationRule): AutomationRule["matcher"] {
    const selectedMethod = String(data.get("method") ?? "GET");
    const method = (
      selectedMethod === "CUSTOM" ? String(data.get("customMethod") ?? "") : selectedMethod
    ).toUpperCase();
    const exact = this.parseEditorHeaders("requestHeaders", data).map((header) => ({
      name: header.name,
      condition: { kind: "exact", value: Array.from(new TextEncoder().encode(header.value)) },
    }));
    const extra = this.retainAdvancedMatcher
      ? (existing?.matcher.requestHeaders.filter((header) => header.condition.kind !== "exact") ??
        [])
      : [];
    const advanced = this.retainAdvancedMatcher ? existing?.matcher : undefined;
    return {
      ...existing?.matcher,
      method: method === "ANY" ? null : method,
      url: this.matchEditor.value,
      examples: this.matchExamples,
      scheme: advanced?.scheme ?? null,
      host: advanced?.host ?? null,
      port: advanced?.port ?? null,
      pathPrefix: advanced?.pathPrefix ?? null,
      query: advanced?.query ?? null,
      requestHeaders: [...exact, ...extra],
      responseHeaders: advanced?.responseHeaders ?? [],
      responseStatus: advanced?.responseStatus ?? null,
      responseStatusClass: advanced?.responseStatusClass ?? null,
    };
  }
  removeAdvancedConditions(): void {
    this.retainAdvancedMatcher = false;
    this.advancedMatcherText = "";
    this.queueResponseDirty();
    this.scheduleMatcherTest();
  }
  private setEditorMethod(method: string | null): void {
    const value = method ?? "ANY";
    const known = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "ANY"].includes(
      value,
    );
    this.autoResponseMethod.value = known ? value : "CUSTOM";
    this.autoResponseCustomMethod.value = known ? "" : value;
    this.customMethodHidden = known;
  }
  private responseValues(): unknown {
    let bodyChanged: boolean;
    try {
      bodyChanged =
        !this.bodyReadOnly &&
        this.bodyOriginalBytes !== null &&
        !equalBodyBytes(this.currentBodyBytes(), this.bodyOriginalBytes);
    } catch {
      bodyChanged = true;
    }
    return {
      fields: this.nonBodySnapshot(),
      bodyChanged,
      path: this.responseBodyPath,
      mode: this.responseBodyPath ? this.responseBodyFileMode : null,
      editBody: this.editingAutoResponseId ? null : this.autoResponseEditBody.checked,
      enabled: this.editingAutoResponseId ? null : this.autoResponseEnabled.checked,
    };
  }
  private captureResponseBaseline(): void {
    this.responseBaseline.capture(this.responseValues());
    this.refreshResponseDirty();
  }
  private refreshResponseDirty(): void {
    if (!this.autoResponseForm || this.editorHidden) {
      return;
    }
    this.draftDirty = this.responseBaseline.differs(this.responseValues());
    if (this.collectionOpen && this.responseExperiment) {
      this.responseSets.markEditorDirty(
        this.draftDirty ||
          [...this.collectionSnapshots.entries()].some(
            ([id, snapshot]) => id !== this.collectionRuleId && snapshot.state.draftDirty === true,
          ),
      );
      const name = (this.autoResponseForm.elements.namedItem("name") as HTMLInputElement).value;
      this.responseSets.updateRuleName(this.collectionRuleId, name);
    }
    this.responseCanSave = !this.existingResponse || this.draftDirty;
  }
  private queueResponseDirty(): void {
    queueMicrotask(() => this.refreshResponseDirty());
  }
  markDraftDirty(event: Event): void {
    const target = event.target as HTMLElement;
    if (["method", "customMethod", "requestHeaders"].includes((target as HTMLInputElement).name)) {
      this.scheduleMatcherTest();
    }
    if (
      target.closest("[data-matcher-test],[data-placeholder-editor],[data-body-view]") ||
      (target instanceof HTMLInputElement &&
        target.name === "enabled" &&
        this.editingAutoResponseId)
    ) {
      return;
    }
    if (target === this.autoResponseMediaType) {
      this.updateLinkedTextStatus();
    }
    this.queueResponseDirty();
  }
  onMatcherChange(): void {
    this.queueResponseDirty();
    this.scheduleMatcherTest();
  }
  scheduleMatcherTest(): void {
    window.clearTimeout(this.matcherTimer);
    this.matcherTestRevision++;
    this.matchTestResult = null;
    this.renderMatchCases();
    this.matcherTimer = window.setTimeout(() => {
      void this.testMatcher();
    }, 300);
  }
  formKeyboard(event: KeyboardEvent): void {
    if (this.collectionOpen && this.responseSets.tourStep > 0) {
      this.responseSets.guideKeyboard(event);
      if (event.defaultPrevented) {
        return;
      }
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      event.stopPropagation();
      if (this.collectionOpen) {
        void this.saveCollection();
      } else {
        this.autoResponseForm.requestSubmit();
      }
    }
  }
  previewSavedRule(mode: "captured" | "modified" = "modified"): void {
    this.closeResponseMenu(this.previewResponseMenu, this.previewResponseButton);
    if (mode === "modified" && !this.editingAutoResponseId) {
      return;
    }
    if (this.responseExperiment && this.responseSets.dirty) {
      this.autoResponseError = `${this.responseSaveLabel} before previewing. Your draft is preserved.`;
      return;
    }
    const revision = this.editorRevision,
      section = this.automationSection,
      dirty = this.draftDirty;
    this.$emit("page-preview-request", {
      ruleId: this.editingAutoResponseId,
      sourceSessionId: this.sourceSessionId,
      mode,
      ...(this.editingVariant
        ? { responseSetId: this.editingVariant.experimentId, variant: this.editingVariant }
        : {}),
      isCurrent: () =>
        this.view === "automation" &&
        section === this.automationSection &&
        revision === this.editorRevision &&
        dirty === this.draftDirty &&
        !this.editorHidden,
    } satisfies ResponsePreviewIntent);
  }
  async canLeaveEditor(): Promise<boolean> {
    if (this.collectionAdding || this.resolvingUnsaved) {
      return false;
    }
    if (this.bodyEditor?.surface?.open && !this.bodyEditor.bodyBusy) {
      return false;
    }
    if (this.collectionOpen) {
      this.responseSets.dismissGuideForNavigation();
      this.captureCollectionResponse();
      if (this.responseSets.busy || this.loadingSavedResponse || this.fileChoiceBusy) {
        return false;
      }
      if (!this.responseSets.dirty) {
        return true;
      }
      if (this.leaveEditorPromise) {
        return this.leaveEditorPromise;
      }
      this.leaveEditorPromise = new Promise((resolve) => {
        this.resolveLeave = resolve;
      });
      this.unsavedDialog.showModal();
      return this.leaveEditorPromise;
    }
    if (this.savingAutoResponse) {
      return false;
    }
    if (this.bodyEditor?.surface?.open && !this.bodyEditor.bodyBusy) {
      return false;
    }
    if (!this.draftDirty || this.editorHidden) {
      return true;
    }
    if (this.leaveEditorPromise) {
      return this.leaveEditorPromise;
    }
    this.leaveEditorPromise = new Promise((resolve) => {
      this.resolveLeave = resolve;
    });
    this.unsavedDialog.showModal();
    return this.leaveEditorPromise;
  }
  @observable resolvingUnsaved = false;
  async resolveUnsaved(choice: string): Promise<void> {
    if (this.resolvingUnsaved || this.savingAutoResponse || this.responseSets.busy) {
      return;
    }
    const collection = this.collectionOpen;
    if (choice === "save") {
      if (!collection && !this.autoResponseForm.checkValidity()) {
        await this.resolveUnsaved("stay");
        this.autoResponseForm.reportValidity();
        return;
      }
      this.resolvingUnsaved = true;
      try {
        if (
          !(await (collection
            ? this.saveCollection()
            : this.saveAutoResponse(new Event("submit", { cancelable: true }))))
        ) {
          this.$update();
          return;
        }
      } finally {
        this.resolvingUnsaved = false;
      }
    }
    if (choice === "discard" && !collection && !this.intakeSourceDiscardDeferred) {
      this.draftDirty = false;
    }
    this.unsavedDialog.close();
    const resolve = this.resolveLeave;
    this.resolveLeave = null;
    this.leaveEditorPromise = null;
    resolve?.(choice !== "stay");
  }
  unsavedCancelled(event: Event): void {
    event.preventDefault();
    void this.resolveUnsaved("stay");
  }
  private nonBodySnapshot(): string {
    const value = (name: string) =>
      (
        this.autoResponseForm.elements.namedItem(name) as
          HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement
      ).value;
    const selected = value("method"),
      method = (selected === "CUSTOM" ? value("customMethod") : selected).toUpperCase();
    const headers = (name: string) => {
      try {
        return parseHeaderLines(value(name)).filter(
          (header) =>
            name !== "responseHeaders" ||
            !["content-length", "content-encoding", "content-type"].includes(
              header.name.toLowerCase(),
            ),
        );
      } catch {
        return value(name);
      }
    };
    return JSON.stringify({
      name: value("name").trim(),
      method,
      url: this.matchEditor.captureDraft(),
      requestHeaders: headers("requestHeaders"),
      retainAdvanced: this.retainAdvancedMatcher,
      examples: this.matchExamples,
      status: Number(value("status")),
      mediaType: optionalText(value("mediaType")),
      responseHeaders: headers("responseHeaders"),
      compression: this.autoResponsePreserveEncoding.checked,
    });
  }
  openBodyDialog(): void {
    if (
      this.savingAutoResponse ||
      this.loadingSavedResponse ||
      this.fileChoiceBusy ||
      this.responseBodyPath ||
      this.editBodyDisabled
    ) {
      return;
    }
    let bytes: number[];
    try {
      bytes = this.currentBodyBytes();
    } catch (error: unknown) {
      this.autoResponseError = describeError(error);
      return;
    }
    this.bodyDialogBefore = {
      revision: this.editorRevision,
      value: this.autoResponseBody.value,
      mode: this.bodyMode,
      viewValue: this.bodyViewValue,
      viewBytes: this.bodyViewBytes,
      status: this.bodyStatusText,
      edit: this.autoResponseEditBody.checked,
      readOnly: this.bodyReadOnly,
      dirty: this.draftDirty,
      error: this.autoResponseError,
    };
    this.bodyDialogCommitted = false;
    const canSave = Boolean(this.editingAutoResponseId && this.savedResponse);
    this.bodyEditor.open(
      this.autoResponseBody.value,
      responseBodyLanguage(this.autoResponseMediaType.value, this.autoResponseBody.value),
      canSave,
      this.bodyDialogButton,
      {
        mode: this.bodyMode,
        textEncoding: this.bodyTextEncoding,
        bytes,
        saveLabel: this.responseSaveLabel,
        saveDescription: this.responseExperiment
          ? `${this.responseSaveLabel} saves all pending response changes. Live traffic is unchanged.`
          : "Save rule saves all pending rule changes.",
        saveUnavailableReason: this.responseExperiment
          ? "Apply to draft, then " + this.responseSaveLabel.toLowerCase() + "."
          : this.editingAutoResponseId
            ? "Apply to draft, then save the rule."
            : "Apply to draft, then save the new rule.",
      },
    );
  }
  async commitBodyDialog(event: CustomEvent<BodyCommit>): Promise<void> {
    const before = this.bodyDialogBefore;
    if (!before || before.revision !== this.editorRevision) {
      this.bodyEditor.bodyError =
        "The selected rule changed. Close this editor and reopen it for the current rule.";
      return;
    }
    const { value, save, mode } = event.detail;
    const bytes = event.detail.bytes ?? encodeBody(value, mode, this.bodyTextEncoding),
      bodyChanged = !equalBodyBytes(bytes, this.currentBodyBytes());
    if (value !== this.autoResponseBody.value || mode !== this.bodyMode || bodyChanged) {
      this.bodyMode = mode;
      this.autoResponseBody.value = value;
      this.bodyViewValue = this.autoResponseBody.value;
      this.bodyViewBytes = bytes;
      if (bodyChanged) {
        this.queueResponseDirty();
        if (this.autoResponseSourceState?.kind === "captured") {
          this.autoResponseEditBody.checked = true;
          this.updateAutoResponseBodyEditState();
        }
      }
    }
    this.updateBodySummary();
    if (!save) {
      this.bodyDialogCommitted = true;
      this.bodyEditor.close();
      return;
    }
    if (!this.bodyEditor.bodyCanSave) {
      this.bodyEditor.bodyError = "Apply the body, then save the new rule.";
      return;
    }
    this.refreshResponseDirty();
    this.bodyEditor.bodyBusy = true;
    this.bodyEditor.bodyError = "";
    let saved = false;
    try {
      if (this.autoResponseForm.reportValidity()) {
        saved = await this.saveAutoResponse(new Event("submit", { cancelable: true }));
      }
    } finally {
      this.bodyEditor.bodyBusy = false;
    }
    if (saved) {
      this.bodyDialogCommitted = true;
      this.bodyEditor.close();
    } else {
      this.bodyEditor.bodyError =
        this.autoResponseError ||
        "The rule could not be saved. Apply the body and review the rule properties.";
    }
  }
  bodyDialogClosed(): void {
    const before = this.bodyDialogBefore;
    if (before && !this.bodyDialogCommitted && before.revision === this.editorRevision) {
      this.bodyMode = before.mode;
      this.autoResponseBody.value = before.value;
      this.bodyViewValue = before.viewValue;
      this.bodyViewBytes = before.viewBytes;
      this.bodyStatusText = before.status;
      this.autoResponseEditBody.checked = before.edit;
      this.bodyReadOnly = before.readOnly;
      this.draftDirty = before.dirty;
      this.autoResponseError = before.error;
      this.updateCompressionChoice();
      this.updateBodySummary();
    }
    this.bodyDialogBefore = null;
    this.bodyDialogCommitted = false;
    if (
      this.responseExperiment &&
      this.editorHidden &&
      this.view === "automation" &&
      this.automationSection === "experiments"
    ) {
      this.responseSets.addButton.focus();
    }
  }
  private currentBodyBytes(): number[] {
    return this.bodyViewBytes !== null && this.autoResponseBody.value === this.bodyViewValue
      ? this.bodyViewBytes
      : encodeBody(this.autoResponseBody.value, this.bodyMode, this.bodyTextEncoding);
  }
  private setBodyContent(bytes: number[], encoding: string | null, display?: string): void {
    this.bodyTextEncoding = encoding;
    this.bodyMode = encoding === null ? "hex" : "text";
    this.bodyOriginalBytes = bytes;
    this.autoResponseBody.value =
      this.bodyMode === "text" && display !== undefined
        ? display
        : renderBodyBytes(bytes, this.bodyMode, encoding);
    this.bodyViewValue = this.autoResponseBody.value;
    this.bodyViewBytes = bytes;
    this.updateBodySummary();
  }
  changeBodyMode(event: Event): void {
    const select = event.currentTarget as HTMLSelectElement,
      mode = select.value as BodyMode;
    if (mode !== "text" && mode !== "hex") {
      return;
    }
    try {
      const bytes = this.currentBodyBytes();
      const value = renderBodyBytes(bytes, mode, this.bodyTextEncoding);
      this.bodyMode = mode;
      this.autoResponseBody.value = value;
      this.bodyViewValue = this.autoResponseBody.value;
      this.bodyViewBytes = bytes;
      this.autoResponseError = "";
      this.updateBodySummary();
    } catch (error: unknown) {
      select.value = this.bodyMode;
      this.autoResponseError = describeError(error);
    }
  }
  updateBodySummary(): void {
    try {
      const bytes = this.currentBodyBytes();
      this.bodyByteSummary = `${this.bodyMode === "hex" ? "Hex" : "Text"} · ${formatBytes(bytes.length)}${this.bodyMode === "hex" ? " · Enter byte pairs (00–FF)." : ""}`;
    } catch (error: unknown) {
      this.bodyByteSummary = describeError(error);
    }
  }
  async revertAutoResponse(): Promise<void> {
    if (this.collectionOpen) {
      await this.discardCollection();
      return;
    }
    const selection = this.editingVariant;
    const rules = selection
      ? this.automationStatus?.responseSets?.find((set) => set.id === selection.experimentId)?.rules
      : this.automationStatus?.rules;
    const rule = rules?.find((rule) => rule.id === this.editingAutoResponseId);
    this.draftDirty = false;
    const captured = this.capturedDraft,
      experiment = this.responseExperiment;
    if (rule) {
      await this.editAutoResponse(rule, true, experiment, selection ?? undefined);
    } else if (captured) {
      await this.populateCapturedAutoResponse(
        captured.sessionId,
        captured.detail,
        captured,
        experiment,
        selection ?? undefined,
      );
    } else if (this.capturedOrigin) {
      await this.populateCapturedAutoResponse(
        this.capturedOrigin.sessionId,
        this.capturedOrigin.detail,
        undefined,
        experiment,
        selection ?? undefined,
      );
    } else {
      await this.beginScratchAutoResponse();
    }
  }
  async setEditorEnabled(): Promise<void> {
    this.scheduleMatcherTest();
    const id = this.editingAutoResponseId;
    if (!id) {
      this.queueResponseDirty();
      return;
    }
    await this.mutateAutoResponse(id, "toggle");
    this.autoResponseEnabled.checked =
      this.automationStatus?.rules.find((rule) => rule.id === id)?.enabled ?? true;
    this.scheduleMatcherTest();
  }
  private ruleOrder(): string[] {
    return [...this.visibleRules, ...this.nativeRuleRows].map((rule) => rule.id);
  }
  preferencesChanged(): void {
    this.ruleListSplit = this.preferences.autoresponseSplit ?? 45;
    this.ruleListShare = this.ruleListSplit + "%";
  }
  private renderRuleSelection(): void {
    this.ruleSelectionCount = this.ruleSelection.ids.size;
    this.ruleSelectionText = this.ruleSelectionCount
      ? `${this.ruleSelectionCount} rule${this.ruleSelectionCount === 1 ? "" : "s"} selected`
      : "Select a rule to edit its properties.";
    this.hasNativeSelection = [...this.ruleSelection.ids].some(
      (id) => !this.rules.some((rule) => rule.id === id),
    );
    this.rules = this.rules.map((rule) => ({
      ...rule,
      selected: this.ruleSelection.ids.has(rule.id),
      selectionState: String(this.ruleSelection.ids.has(rule.id)),
      tabIndex: this.ruleSelection.focused === rule.id ? "0" : "-1",
    }));
    const search = this.ruleSearch.toLowerCase();
    this.visibleRules = this.rules.filter(
      (rule) =>
        this.ruleDirection !== "requests" &&
        (!search ||
          (rule.name + " " + rule.criteria + " Serve a saved response")
            .toLowerCase()
            .includes(search)) &&
        (this.ruleFilter === "all" ||
          (this.ruleFilter === "enabled" && rule.enabled !== false) ||
          (this.ruleFilter === "disabled" && rule.enabled === false) ||
          (this.ruleFilter === "shadowed" && Boolean(rule.shadowedBy))),
    );
    if (!this.visibleRules.some((rule) => rule.tabIndex === "0") && this.visibleRules[0]) {
      this.visibleRules = this.visibleRules.map((rule, index) => ({
        ...rule,
        tabIndex: index === 0 ? "0" : "-1",
      }));
    }
    this.renderNativeRules();
    const order = this.ruleOrder();
    this.allRulesSelected = order.length > 0 && order.every((id) => this.ruleSelection.ids.has(id));
  }
  searchRules(event: Event): void {
    this.finishRuleDrag();
    this.ruleSearch = (event.target as HTMLInputElement).value;
    this.ruleSelection.clear();
    this.renderRuleSelection();
  }
  filterRules(event: Event): void {
    this.finishRuleDrag();
    this.ruleFilter = (event.target as HTMLSelectElement).value;
    this.ruleSelection.clear();
    this.renderRuleSelection();
  }
  async selectRule(rule: AutomationRule, event?: MouseEvent, checkbox = false): Promise<void> {
    if (this.savingAutoResponse || this.changingAutoResponse) {
      return;
    }
    this.catalogSelectedId = "";
    this.nativeDetail = false;
    this.finishRuleDrag();
    const previous = this.ruleSelection.clone(),
      next = this.ruleSelection.clone();
    next.choose(
      rule.id,
      this.ruleOrder(),
      checkbox ? { ctrlKey: true, shiftKey: event?.shiftKey ?? false } : (event ?? {}),
    );
    if (
      (next.ids.size !== 1 || !next.ids.has(this.editingAutoResponseId ?? "")) &&
      !(await this.canLeaveEditor())
    ) {
      return;
    }
    this.ruleSelection = next;
    this.batchReviewHidden = true;
    this.renderRuleSelection();
    if (next.ids.size === 1) {
      const selected = this.automationStatus?.rules.find((rule) => next.ids.has(rule.id));
      if (selected && !(await this.openRuleProperties(selected))) {
        this.ruleSelection = previous;
        this.renderRuleSelection();
        return;
      }
    } else {
      this.showRuleSelection();
    }
    this.ruleSelection.focused = rule.id;
    this.renderRuleSelection();
    this.focusRule(rule.id);
  }
  selectRuleCheckbox(rule: AutomationRule, event: MouseEvent): void {
    event.stopPropagation();
    void this.selectRule(rule, event, true);
  }
  async ruleKeyboard(rule: AutomationRule, event: KeyboardEvent): Promise<void> {
    if (this.savingAutoResponse || this.changingAutoResponse) {
      return;
    }
    if (this.rulePointer) {
      event.preventDefault();
      if (event.key === "Escape") {
        this.cancelRulePointer();
      }
      return;
    }
    if (isTextEditing(event)) {
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (!this.editorHidden) {
        this.autoResponseForm.requestSubmit();
      }
      return;
    }
    if (event.key === "Delete") {
      event.preventDefault();
      await this.mutateSelectedRules("remove");
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
      event.preventDefault();
      await this.undoRemoveAutoResponse();
      return;
    }
    if (
      (event.ctrlKey || event.metaKey) &&
      event.altKey &&
      (event.key === "ArrowUp" || event.key === "ArrowDown")
    ) {
      event.preventDefault();
      await this.mutateSelectedRules(event.key === "ArrowUp" ? "up" : "down");
      return;
    }
    const next = this.ruleSelection.clone();
    if (!next.key(rule.id, this.ruleOrder(), event)) {
      return;
    }
    if (
      (next.ids.size !== 1 || !next.ids.has(this.editingAutoResponseId ?? "")) &&
      !(await this.canLeaveEditor())
    ) {
      return;
    }
    this.ruleSelection = next;
    this.renderRuleSelection();
    this.$flushUpdates();
    const focused = this.automationStatus?.rules.find((rule) => rule.id === next.focused);
    if (focused) {
      this.focusRule(focused.id);
      const selected = this.automationStatus?.rules.find((row) => next.ids.has(row.id));
      if (next.ids.size === 1 && selected) {
        await this.openRuleProperties(selected);
      } else if (next.ids.size !== 1) {
        this.showRuleSelection();
      }
      this.ruleSelection.focused = focused.id;
      this.renderRuleSelection();
      this.focusRule(focused.id);
    } else {
      this.editorRevision++;
      this.editorHidden = true;
      this.editingAutoResponseId = null;
    }
  }
  listKeyboard(event: KeyboardEvent): void {
    if (event.key === "Escape" && this.draggedRuleId) {
      event.preventDefault();
      this.cancelRulePointer();
      return;
    }
    if (event.defaultPrevented || isTextEditing(event)) {
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "z") {
      event.preventDefault();
      void this.undoRemoveAutoResponse();
    } else if (event.key === "Delete") {
      event.preventDefault();
      void this.mutateSelectedRules("remove");
    }
  }
  positionRuleMenu(event: MouseEvent): void {
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    this.ruleMenuX = Math.max(10, Math.min(rect.left, window.innerWidth - 260)) + "px";
    this.ruleMenuY = Math.max(50, Math.min(rect.bottom + 4, window.innerHeight - 320)) + "px";
    this.$flushUpdates();
  }
  async openRuleMenu(rule: AutomationRule, event: MouseEvent): Promise<void> {
    event.preventDefault();
    if (!this.ruleSelection.ids.has(rule.id)) {
      await this.selectRule(rule);
    }
    if (this.ruleSelection.ids.has(rule.id)) {
      this.ruleMenuX = Math.max(10, Math.min(event.clientX, window.innerWidth - 260)) + "px";
      this.ruleMenuY = Math.max(50, Math.min(event.clientY, window.innerHeight - 320)) + "px";
      this.$flushUpdates();
      this.ruleMenu.showPopover();
    }
  }
  async selectAllRules(event: Event): Promise<void> {
    const input = event.target as HTMLInputElement,
      checked = input.checked;
    if (!(await this.canLeaveEditor())) {
      input.checked = this.allRulesSelected;
      return;
    }
    this.ruleSelection.ids = new Set(checked ? this.ruleOrder() : []);
    this.renderRuleSelection();
    const selected = this.automationStatus?.rules.find((rule) =>
      this.ruleSelection.ids.has(rule.id),
    );
    if (this.ruleSelectionCount === 1 && selected) {
      await this.openRuleProperties(selected);
    } else {
      this.showRuleSelection();
    }
  }
  resizeRuleList(event: CustomEvent<{ value: number; committed: boolean }>): void {
    this.ruleListSplit = event.detail.value;
    this.ruleListShare = event.detail.value + "%";
    this.$emit("workspace-change", {
      patch: { autoresponseSplit: event.detail.value },
      committed: event.detail.committed,
    });
  }
  private rememberRuleChange(
    before: AutomationRule[],
    after: AutomationRule[],
    label: string,
    all = false,
  ): void {
    this.automationText = "";
    this.automationKind = "success";
    this.ruleHistory.push({
      before: structuredClone(before),
      after: structuredClone(after),
      label,
      all,
    });
    if (this.ruleHistory.length > 20) {
      this.ruleHistory.shift();
    }
    this.undoRemoveText = label;
  }
  private async mutateCatalogSelection(
    mutation: "enable" | "disable" | "remove" | "up" | "down" | "duplicate",
  ): Promise<void> {
    if (mutation === "up" || mutation === "down") {
      this.automationText =
        "Select only saved responses to change their priority here. Other rules have order controls in their properties.";
      return;
    }
    if (mutation === "remove") {
      for (const [editor, section] of [
        [this.headerRulesEditor, "header-rules"],
        [this.responseTextEditor, "response-text"],
      ] as const) {
        if (this.ruleSelection.ids.has(editor.hrId) && editor.hrDirty) {
          const previous = this.automationSection;
          this.showAutomationSection(section);
          this.$flushUpdates();
          const leave = await editor.canLeaveDraft();
          this.showAutomationSection(previous);
          if (!leave) {
            return;
          }
        }
      }
      if (
        this.ruleSelection.ids.has(this.userAgentOverrides.uaSelectedId) &&
        !(await this.userAgentOverrides.canLeaveDraft())
      ) {
        return;
      }
    }
    const selection = this.ruleSelection.clone(),
      section = this.automationSection,
      root = this.getRootNode();
    let restoreFocus = true;
    const moved = (event: Event) => {
      const target = event.composedPath()[0];
      if (
        target instanceof Node &&
        !this.autoResponseList.contains(target) &&
        !this.ruleMenu.contains(target) &&
        target !== this.ruleMoreButton
      ) {
        restoreFocus = false;
      }
    };
    root.addEventListener("focusin", moved);
    this.ruleMenu.hidePopover();
    this.changingAutoResponse = true;
    try {
      const current = await invoke<AutomationStatus>("automation_status"),
        selected = selection.ids;
      let rules = current.rules;
      if (mutation === "remove") {
        rules = rules.filter((rule) => !selected.has(rule.id));
      } else if (mutation === "duplicate") {
        const copies = rules
          .filter((rule) => selected.has(rule.id))
          .map((rule) => ({
            ...structuredClone(rule),
            id: "rule-" + crypto.randomUUID(),
            displayName: Array.from("Copy of " + (rule.displayName ?? "Rule"))
              .slice(0, 128)
              .join(""),
            enabled: false,
            revision: 1,
          }));
        rules = [...rules, ...copies];
        this.ruleSelection.ids = new Set(copies.map((rule) => rule.id));
      } else {
        rules = rules.map((rule) =>
          selected.has(rule.id)
            ? { ...rule, enabled: mutation === "enable", revision: rule.revision + 1 }
            : rule,
        );
      }
      const status = await this.activateRuleSet(rules, current.generation);
      this.rememberRuleChange(
        current.rules,
        status.rules,
        mutation === "remove"
          ? `Removed ${selected.size} rule${selected.size === 1 ? "" : "s"}.`
          : mutation === "duplicate"
            ? "Created disabled copies."
            : "Changed selected rule enabled states.",
        true,
      );
      if (mutation === "remove") {
        this.ruleSelection.clear();
      }
      this.renderAutomation(status);
      if (this.automationSection === section) {
        this.showRuleSelection();
      }
      this.$flushUpdates();
      if (
        mutation === "remove" &&
        restoreFocus &&
        this.view === "automation" &&
        this.automationSection === section
      ) {
        this.undoRemoveButton.focus();
      } else if (
        mutation !== "remove" &&
        this.ruleSelection.ids.size === 1 &&
        this.automationSection === section
      ) {
        const rule = status.rules.find((rule) => this.ruleSelection.ids.has(rule.id));
        if (rule) {
          await this.openRuleProperties(rule);
        }
      }
    } catch (error) {
      this.ruleSelection = selection;
      this.renderRuleSelection();
      this.automationText = "Rule change failed: " + describeError(error);
      this.automationKind = "error";
    } finally {
      root.removeEventListener("focusin", moved);
      this.changingAutoResponse = false;
    }
  }
  async mutateSelectedRules(
    mutation: "enable" | "disable" | "remove" | "up" | "down" | "duplicate",
  ): Promise<void> {
    if (this.savingAutoResponse || this.changingAutoResponse || !this.ruleSelection.ids.size) {
      return;
    }
    if (mutation === "enable" || mutation === "disable") {
      if (this.ruleMenu.matches(":popover-open")) {
        this.ruleMenu.hidePopover();
        this.ruleMoreButton.focus({ preventScroll: true });
      }
      await this.setRulesEnabled(
        (this.automationStatus?.rules ?? []).filter((rule) => this.ruleSelection.ids.has(rule.id)),
        mutation === "enable",
      );
      return;
    }
    if ((mutation === "remove" || mutation === "duplicate") && !(await this.canLeaveEditor())) {
      return;
    }
    if (this.hasNativeSelection) {
      await this.mutateCatalogSelection(mutation);
      return;
    }
    this.ruleMenu.hidePopover();
    this.changingAutoResponse = true;
    const originalSelection = this.ruleSelection.clone();
    try {
      const current = await invoke<AutomationStatus>("automation_status");
      const before = this.autoResponseRules(current),
        selected = this.ruleSelection.ids;
      let ordered = [...before];
      if (mutation === "remove") {
        ordered = ordered.filter((rule) => !selected.has(rule.id));
      } else if (mutation === "duplicate") {
        const copies: string[] = [];
        ordered = ordered.flatMap((rule) => {
          if (!selected.has(rule.id)) {
            return [rule];
          }
          const id = "autoresponse-rule-" + crypto.randomUUID().replaceAll("-", "");
          copies.push(id);
          return [
            rule,
            {
              ...rule,
              id,
              displayName: Array.from("Copy of " + (rule.displayName ?? "Saved response"))
                .slice(0, 128)
                .join(""),
              enabled: false,
              revision: 1,
            },
          ];
        });
        this.ruleSelection.ids = new Set(copies);
      } else if (mutation === "up") {
        for (let index = 1; index < ordered.length; index++) {
          if (selected.has(ordered[index]!.id) && !selected.has(ordered[index - 1]!.id)) {
            [ordered[index - 1], ordered[index]] = [ordered[index]!, ordered[index - 1]!];
          }
        }
      } else {
        for (let index = ordered.length - 2; index >= 0; index--) {
          if (selected.has(ordered[index]!.id) && !selected.has(ordered[index + 1]!.id)) {
            [ordered[index], ordered[index + 1]] = [ordered[index + 1]!, ordered[index]!];
          }
        }
      }
      const label =
        mutation === "remove"
          ? `Removed ${selected.size} rule${selected.size === 1 ? "" : "s"}.`
          : mutation === "duplicate"
            ? "Created disabled copies."
            : mutation === "up" || mutation === "down"
              ? "Changed rule priority."
              : "Changed selected rule enabled states.";
      const status = await this.activateAutoResponseOrder(current, ordered);
      this.rememberRuleChange(before, this.autoResponseRules(status), label);
      this.renderAutomation(status);
      if (mutation === "remove") {
        this.ruleSelection.clear();
        this.editingAutoResponseId = null;
        this.renderRuleSelection();
        this.editorHidden = true;
        this.draftDirty = false;
        this.editorRevision++;
        this.changingAutoResponse = false;
        this.$flushUpdates();
        this.undoRemoveButton.focus();
      } else {
        this.renderRuleSelection();
        const id = [...this.ruleSelection.ids][0];
        if (mutation === "duplicate") {
          this.changingAutoResponse = false;
          this.draftDirty = false;
          if (this.ruleSelection.ids.size === 1 && id) {
            await this.editAutoResponse(
              status.rules.find((rule) => rule.id === id)!,
              true,
            );
          } else {
            this.editorRevision++;
            this.editorHidden = true;
          }
        }
        if (this.editingAutoResponseId) {
          this.autoResponseEnabled.checked =
            status.rules.find((rule) => rule.id === this.editingAutoResponseId)?.enabled ?? true;
        }
        if (id) {
          this.focusRule(id);
        }
      }
    } catch (error: unknown) {
      this.ruleSelection = originalSelection;
      this.renderRuleSelection();
      this.automationText = "Rule change failed: " + describeError(error);
      this.automationKind = "error";
    } finally {
      this.changingAutoResponse = false;
    }
  }
  async beginBatch(ids: string[]): Promise<void> {
    if (!Array.isArray(ids) || ids.length > 256 || ids.some((id) => typeof id !== "string")) {
      this.showNotice(
        "Select fewer responses",
        "Create 1–256 responses per batch. Your Traffic selection is preserved.",
        null,
        null,
      );
      return;
    }
    if (!ids.length || !(await this.canLeaveEditor())) {
      return;
    }
    this.prepareEditor();
    this.editorHidden = true;
    this.batchReviewHidden = false;
    this.autoResponseSourceState = null;
    this.editingAutoResponseId = null;
    const revision = this.editorRevision;
    this.batchLoading = true;
    this.batchError = "";
    this.batchRows = ids.map((id) => ({
      id,
      included: true,
      order: 0,
      name: "Checking response…",
      method: "",
      url: "",
      eligible: false,
      reason: "Checking retention…",
      duplicate: "",
      startedAt: 0,
    }));
    try {
      const status = await invoke<AutomationStatus>("automation_status");
      if (revision !== this.editorRevision) {
        return;
      }
      this.batchGeneration = status.generation;
      for (let offset = 0; offset < ids.length; offset += 8) {
        const results = await Promise.all(
          ids.slice(offset, offset + 8).map(async (id) => {
            try {
              const detail = await invoke<SessionDetail>("session_detail", { id });
              const source = clientResponseSource(detail);
              const request = detail.requests.find((head) => head.boundary === "client-request");
              const method = request?.method ?? "";
              const url = implicitDefaultPort(request?.target ?? "");
              return {
                id,
                included: true,
                order: 0,
                name: method + " " + shortUrl(url),
                method,
                url,
                eligible: Boolean(source),
                reason: source ? "Ready" : autoResponseUnavailableReason(detail),
                duplicate: "",
                startedAt: detail.startedAt ?? 0,
              };
            } catch (error: unknown) {
              return {
                id,
                included: true,
                order: 0,
                name: "Unavailable response",
                method: "",
                url: "",
                eligible: false,
                reason: describeError(error),
                duplicate: "",
                startedAt: 0,
              };
            }
          }),
        );
        if (revision !== this.editorRevision) {
          return;
        }
        const updates = new Map(results.map((row) => [row.id, row]));
        this.batchRows = this.batchRows.map((row) => updates.get(row.id) ?? row);
        this.renderBatchReview();
      }
      if (revision === this.editorRevision) {
        this.batchLoading = false;
        this.renderBatchReview();
      }
    } catch (error: unknown) {
      if (revision === this.editorRevision) {
        this.batchError = describeError(error);
      }
    } finally {
      if (revision === this.editorRevision) {
        this.batchLoading = false;
      }
    }
  }
  private renderBatchReview(): void {
    const seen = new Set<string>();
    this.batchRows = this.batchRows.map((row, index) => {
      const key = row.method + " " + row.url;
      let duplicate =
        row.eligible && row.included && seen.has(key)
          ? "Always superseded by an earlier selected response with the same match."
          : "";
      if (!duplicate && row.eligible && row.included) {
        const existing = this.automationStatus?.rules.find(
          (rule) =>
            rule.request.responseAsset &&
            rule.matcher.method === row.method &&
            rule.matcher.url?.kind === "exact" &&
            implicitDefaultPort(rule.matcher.url.value) === row.url &&
            rule.matcher.requestHeaders.length === 0 &&
            !rule.matcher.host &&
            !rule.matcher.scheme &&
            !rule.matcher.port &&
            !rule.matcher.pathPrefix &&
            rule.matcher.query === null,
        );
        if (existing) {
          duplicate = `Will have higher priority than existing “${existing.displayName ?? "Saved response"}” with the same match.`;
        }
      }
      if (row.eligible && row.included) {
        seen.add(key);
      }
      return { ...row, order: index + 1, duplicate };
    });
    this.batchEligibleCount = this.batchRows.filter((row) => row.eligible && row.included).length;
    const unavailable = this.batchRows.filter((row) => !row.eligible).length;
    const excluded = this.batchRows.filter((row) => row.eligible && !row.included).length;
    this.batchReviewText = this.batchLoading
      ? `${this.batchRows.length} selected · Checking retained responses…`
      : `${this.batchRows.length} selected · ${this.batchEligibleCount} ready${unavailable ? ` · ${unavailable} unavailable` : ""}${excluded ? ` · ${excluded} excluded` : ""}. Rules are added at the top in the order below.`;
  }
  skipBatchRow(id: string): void {
    this.batchRows = this.batchRows.map((row) =>
      row.id === id ? { ...row, included: false } : row,
    );
    this.renderBatchReview();
  }
  moveBatchRow(id: string, direction: number): void {
    const rows = [...this.batchRows],
      index = rows.findIndex((row) => row.id === id),
      next = index + direction;
    if (next < 0 || next >= rows.length) {
      return;
    }
    [rows[index], rows[next]] = [rows[next]!, rows[index]!];
    this.batchRows = rows;
    this.renderBatchReview();
  }
  keepFirstBatchMatches(): void {
    const seen = new Set<string>();
    this.batchRows = this.batchRows.map((row) => {
      const key = row.method + " " + row.url;
      const included = row.eligible && !seen.has(key);
      if (included) {
        seen.add(key);
      }
      return { ...row, included };
    });
    this.renderBatchReview();
  }
  keepNewestBatchMatches(): void {
    const newest = new Map<string, BatchRow>();
    for (const row of this.batchRows) {
      if (row.eligible) {
        const key = row.method + " " + row.url;
        const previous = newest.get(key);
        if (!previous || row.startedAt > previous.startedAt) {
          newest.set(key, row);
        }
      }
    }
    this.batchRows = this.batchRows.map((row) => ({
      ...row,
      included: row.eligible && newest.get(row.method + " " + row.url) === row,
    }));
    this.renderBatchReview();
  }
  async reviewBatchAgain(): Promise<void> {
    const choices = new Map(this.batchRows.map((row) => [row.id, row.included]));
    await this.beginBatch(this.batchRows.map((row) => row.id));
    if (this.batchReviewHidden) {
      return;
    }
    this.batchRows = this.batchRows.map((row) => ({
      ...row,
      included: choices.get(row.id) ?? row.included,
    }));
    this.renderBatchReview();
  }
  cancelBatch(): void {
    if (this.savingAutoResponse) {
      return;
    }
    this.editorRevision++;
    this.batchReviewHidden = true;
    this.batchLoading = false;
    this.newRuleButton.focus();
  }
  async createBatchRules(): Promise<void> {
    if (this.savingAutoResponse || this.batchLoading || !this.batchEligibleCount) {
      return;
    }
    this.savingAutoResponse = true;
    this.batchError = "";
    try {
      const before = this.automationStatus ? this.autoResponseRules(this.automationStatus) : [];
      const result = await invoke<{ status: AutomationStatus; createdIds: string[] }>(
        "create_autoresponse_batch",
        {
          input: {
            ids: this.batchRows.filter((row) => row.eligible && row.included).map((row) => row.id),
            generation: this.batchGeneration,
          },
        },
      );
      this.rememberRuleChange(
        before,
        this.autoResponseRules(result.status),
        `Created ${result.createdIds.length} rules.`,
      );
      this.renderAutomation(result.status);
      await this.refreshAutomation();
      this.ruleSelection.ids = new Set(result.createdIds);
      this.ruleSelection.focused = result.createdIds[0] ?? null;
      this.batchReviewHidden = true;
      this.renderRuleSelection();
      this.savingAutoResponse = false;
      const id = result.createdIds[0];
      if (id) {
        this.focusRule(id);
        if (result.createdIds.length === 1) {
          await this.editAutoResponse(this.rules.find((rule) => rule.id === id)!);
        }
      }
    } catch (error: unknown) {
      this.batchError = describeError(error);
    } finally {
      this.savingAutoResponse = false;
    }
  }
  async testMatcher(): Promise<void> {
    window.clearTimeout(this.matcherTimer);
    const revision = ++this.matcherTestRevision;
    this.matcherTestBusy = true;
    this.matcherTestError = "";
    try {
      const existing =
        this.automationStatus?.rules.find((rule) => rule.id === this.editingAutoResponseId) ??
        this.responseRuleTemplate ??
        undefined;
      const matcher = this.currentMatcher(new FormData(this.autoResponseForm), existing);
      let validation: MatchTestResult;
      try {
        validation = await invoke<MatchTestResult>("test_autoresponse_match", {
          input: {
            matcher,
            method: matcher.method ?? "GET",
            url: "https://example.test/",
            headers: [],
            ruleId: this.editingAutoResponseId,
            enabled: this.autoResponseEnabled.checked,
          },
        });
      } catch (error: unknown) {
        if (revision === this.matcherTestRevision) {
          this.matchEditor.matchValidationError = describeError(error);
        }
        throw error;
      }
      if (revision !== this.matcherTestRevision) {
        return;
      }
      this.matchEditor.matchValidationError = "";
      if (!this.matchTestUrl.value) {
        this.matchTestResult = null;
        this.renderMatchCases(validation);
        return;
      }
      const headers = parseHeaderLines(this.matchTestHeaders.value);
      const result = await invoke<MatchTestResult>("test_autoresponse_match", {
        input: {
          matcher,
          method: this.matchTestMethod.value || "GET",
          url: this.matchTestUrl.value,
          headers,
          ruleId: this.editingAutoResponseId,
          enabled: this.autoResponseEnabled.checked,
        },
      });
      if (revision === this.matcherTestRevision && !this.editorHidden) {
        this.matchTestResult = result;
        this.renderMatchCases(result);
        if (!this.matchExpectationChosen) {
          this.matchExpected = result.test.matched ? "match" : "nonmatch";
        }
      }
    } catch (error: unknown) {
      if (revision === this.matcherTestRevision) {
        this.matcherTestError = describeError(error);
        this.matchTestResult = null;
        this.renderMatchCases();
      }
    } finally {
      if (revision === this.matcherTestRevision) {
        this.matcherTestBusy = false;
      }
    }
  }
  addMatchExample(): void {
    if (!this.matchTestUrl.value || !this.matchTestResult || this.matcherTestBusy) {
      return;
    }
    let headers: Array<{ name: string; value: string }>;
    try {
      headers = parseHeaderLines(this.matchTestHeaders.value);
    } catch (error: unknown) {
      this.matcherTestError = describeError(error);
      return;
    }
    const example = {
      method: this.matchTestMethod.value || "GET",
      url: this.matchTestUrl.value,
      expected: this.matchExpected === "match",
      headers,
    };
    const identity = (item: MatchExample) =>
      JSON.stringify([
        item.method,
        item.url,
        (item.headers ?? []).map((header) => [header.name.toLowerCase(), header.value]),
      ]);
    const index = this.matchExamples.findIndex((item) => identity(item) === identity(example));
    if (index < 0 && this.matchExamples.length >= 32) {
      this.matcherTestError = "This rule has 32 test cases. Remove a case before adding another.";
      return;
    }
    this.queueResponseDirty();
    this.matchExamples =
      index < 0
        ? [...this.matchExamples, example]
        : this.matchExamples.map((item, position) => (position === index ? example : item));
    void this.testMatcher();
  }
  removeMatchExample(index: number): void {
    this.queueResponseDirty();
    this.matchExamples = this.matchExamples.filter((_, position) => position !== index);
    void this.testMatcher();
  }
  async chooseResponseBody(): Promise<void> {
    if (this.savingAutoResponse || this.fileChoiceBusy) {
      return;
    }
    const revision = this.editorRevision,
      request = ++this.fileInfoRevision;
    this.fileChoiceBusy = true;
    try {
      const path = await invoke<string | null>("pick_response_body");
      if (
        !path ||
        revision !== this.editorRevision ||
        request !== this.fileInfoRevision ||
        !this.isConnected
      ) {
        return;
      }
      const info = await invoke<ResponseBodyFileInfo>("response_body_file_info", {
        path,
        reference: null,
      });
      if (
        revision !== this.editorRevision ||
        request !== this.fileInfoRevision ||
        !this.isConnected
      ) {
        return;
      }
      this.queueResponseDirty();
      this.bodyFileTouched = true;
      this.setResponseFile(info);
    } catch (error: unknown) {
      if (revision === this.editorRevision && request === this.fileInfoRevision) {
        this.autoResponseError = describeError(error);
      }
    } finally {
      if (request === this.fileInfoRevision) {
        this.fileChoiceBusy = false;
      }
    }
  }
  private setResponseFile(info: ResponseBodyFileInfo): void {
    this.responseBodyPath = info.path;
    this.responseBodyFile = info.path;
    this.responseBodyFileBytes = info.bytes;
    this.renderResponseFileDetails();
    this.responseBodyFileError = info.error ?? "";
    this.updateCompressionChoice();
  }
  changeResponseBodyFileMode(event: Event): void {
    const mode = (event.currentTarget as HTMLSelectElement).value;
    if (!this.responseBodyPath || (mode !== "copy" && mode !== "linked")) {
      return;
    }
    this.responseBodyFileMode = mode;
    this.bodyFileTouched = true;
    this.queueResponseDirty();
    this.renderResponseFileDetails();
  }
  private renderResponseFileDetails(): void {
    const size =
      this.responseBodyFileBytes === null
        ? ""
        : formatBytes(this.responseBodyFileBytes) + " currently available. ";
    this.responseBodyFileDetails =
      size +
      (this.responseBodyFileMode === "linked"
        ? "Read directly for each request. External edits affect future live responses and newly opened previews. F5 keeps the open preview’s frozen bytes."
        : "Contents will be copied when you save. External edits will not affect the saved response.");
  }
  private async refreshResponseFile(): Promise<void> {
    const path = this.responseBodyPath,
      revision = this.editorRevision;
    if (!path || this.fileChoiceBusy) {
      return;
    }
    const request = ++this.fileInfoRevision;
    this.fileChoiceBusy = true;
    try {
      const info = await invoke<ResponseBodyFileInfo>("response_body_file_info", {
        path,
        reference: this.bodyFileTouched
          ? null
          : this.savedResponse?.asset.linkedBody
            ? `${this.savedResponse.asset.id}@${this.savedResponse.asset.revision}`
            : null,
      });
      if (
        revision === this.editorRevision &&
        request === this.fileInfoRevision &&
        path === this.responseBodyPath
      ) {
        this.setResponseFile(info);
      }
    } catch (error: unknown) {
      if (
        revision === this.editorRevision &&
        request === this.fileInfoRevision &&
        path === this.responseBodyPath
      ) {
        this.responseBodyFileBytes = null;
        this.renderResponseFileDetails();
        this.responseBodyFileError = describeError(error);
      }
    } finally {
      // The request owns busy even after departure invalidates its editor data.
      if (request === this.fileInfoRevision) {
        this.fileChoiceBusy = false;
      }
    }
  }
  clearResponseBodyFile(): void {
    this.fileInfoRevision++;
    this.fileChoiceBusy = false;
    this.responseBodyFileMode = "copy";
    this.responseBodyFileBytes = null;
    this.queueResponseDirty();
    this.bodyFileTouched = Boolean(this.savedResponse?.asset.linkedBody);
    this.responseBodyPath = null;
    this.responseBodyFile = "";
    this.responseBodyFileDetails = "";
    this.responseBodyFileError = "";
    if (this.savedResponse?.asset.linkedBody) {
      this.bodyReadOnly = false;
      this.editBodyDisabled = false;
      this.setBodyContent([], responseTextEncoding(this.autoResponseMediaType.value));
      this.updateLinkedTextStatus();
    }
    this.updateCompressionChoice();
  }
  private updateLinkedTextStatus(): void {
    if (!this.savedResponse?.asset.linkedBody || this.responseBodyPath) {
      return;
    }
    const encoding = responseTextEncoding(this.autoResponseMediaType.value);
    this.bodyTextEncoding = encoding;
    this.bodyStatusText = encoding
      ? `Enter a replacement text body, or choose another file. Text uses ${encoding.toUpperCase()}.`
      : "To save text, set a supported charset in Content type (for example, UTF-8), or choose another file.";
  }
  private editableHeaders(asset: Pick<ResponseAsset, "headers">): string {
    this.originalResponseHeaders = [];
    const lines: string[] = [];
    const decoder = new TextDecoder("utf-8", { fatal: true });
    for (const field of asset.headers ?? []) {
      const name = new TextDecoder().decode(new Uint8Array(field.name));
      if (["content-length", "content-encoding", "content-type"].includes(name.toLowerCase())) {
        continue;
      }
      let line: string | null = null;
      try {
        line = name + ": " + decoder.decode(new Uint8Array(field.value));
        lines.push(line);
      } catch {
        /* Non-text values stay in their original ordered slots. */
      }
      this.originalResponseHeaders.push({ field, line });
    }
    return lines.join("\n");
  }
  private mergeResponseHeaders(
    text: string,
    edited: Array<{ name: string; value: string }>,
  ): ResponseAsset["headers"] {
    if (text === this.savedHeaderText) {
      return this.originalResponseHeaders.map(({ field }) => field);
    }
    const result = encodeHeaders(edited),
      available = new Set(edited.map((_, index) => index));
    const byLine = new Map<string, number[]>(),
      byName = new Map<string, number[]>();
    for (let index = edited.length - 1; index >= 0; index--) {
      const header = edited[index]!;
      for (const [map, key] of [
        [byLine, header.name.toLowerCase() + ": " + header.value],
        [byName, header.name.toLowerCase()],
      ] as const) {
        const queue = map.get(key) ?? [];
        queue.push(index);
        map.set(key, queue);
      }
    }
    const take = (queue: number[] | undefined): number | undefined => {
      while (queue?.length) {
        const index = queue.pop()!;
        if (available.delete(index)) {
          return index;
        }
      }
      return undefined;
    };
    // Match unchanged fields first (including duplicates), then edited values by name.
    // A protected field stays before its next surviving text field; deleting an earlier
    // text field therefore cannot swap repeated values across the protected field.
    const matches = this.originalResponseHeaders.map(({ line }) => {
      if (line === null) {
        return undefined;
      }
      const header = parseHeaderLines(line)[0]!;
      return take(byLine.get(header.name.toLowerCase() + ": " + header.value));
    });
    this.originalResponseHeaders.forEach(({ field, line }, index) => {
      if (line === null) {
        return;
      }
      if (matches[index] !== undefined) {
        const match = matches[index]!;
        result[match] = {
          name:
            edited[match]!.name === line.slice(0, line.indexOf(":"))
              ? field.name
              : result[match]!.name,
          value: field.value,
        };
      } else {
        matches[index] = take(
          byName.get(new TextDecoder().decode(new Uint8Array(field.name)).toLowerCase()),
        );
      }
    });
    const protectedSlots = new Map<number, ResponseAsset["headers"]>();
    let next = result.length;
    for (let index = this.originalResponseHeaders.length - 1; index >= 0; index--) {
      const { field, line } = this.originalResponseHeaders[index]!;
      if (line !== null) {
        if (matches[index] !== undefined) {
          next = matches[index]!;
        }
      } else {
        const slot = protectedSlots.get(next) ?? [];
        slot.unshift(field);
        protectedSlots.set(next, slot);
      }
    }
    return result
      .flatMap((field, index) => [...(protectedSlots.get(index) ?? []), field])
      .concat(protectedSlots.get(result.length) ?? []);
  }
  private prepareEditor(experiment = false): void {
    // Callers resolve the draft boundary before replacing its editor. Ordinary
    // rule reveals must also release collection ownership of Save and navigation.
    if (!experiment && this.collectionOpen) {
      this.collectionSnapshots.clear();
      this.collectionRuleId = "";
      this.collectionOpen = false;
      this.collectionName = "";
      this.responseSets.close(false);
    }
    this.responseRuleTemplate = null;
    this.nativeDetail = false;
    this.catalogSelectedId = "";
    this.automationSection = experiment ? "experiments" : "responses";
    this.responseExperiment = experiment;
    this.experimentLive = false;
    this.experimentStateText = "";
    this.editingVariant = null;
    this.variantHistorical = false;
    this.responseSaveLabel = experiment ? "Save changes" : "Save rule";
    this.responseNameLabel = experiment ? "Response name" : "Rule name";
    this.fileInfoRevision++;
    this.responseBodyFileMode = "copy";
    this.responseCopiedFrom = "";
    this.responseBodyFileBytes = null;
    this.editorRevision++;
    if (this.editorHidden) {
      const opener = (this.getRootNode() as ShadowRoot).activeElement as HTMLElement | null;
      this.editorOpener = opener?.closest("#rule-kind-menu") ? this.newRuleButton : opener;
    }
    this.responseBaseline = new DraftBaseline();
    this.autoResponseForm.reset();
    this.matchTestMethod.value = "GET";
    this.matchExpectationChosen = false;
    this.matchExpected = "match";
    this.matchCaseRows = [];
    this.draftDirty = false;
    this.batchReviewHidden = true;
    this.retainAdvancedMatcher = true;
    this.advancedMatcherText = "";
    this.ruleDiagnosticText = "";
    this.ruleSupersededBy = "";
    this.customMethodHidden = true;
    this.batchLoading = false;
    this.loadingSavedResponse = false;
    this.savedResponse = null;
    this.responseBodyPath = null;
    this.responseBodyFile = "";
    this.existingResponse = false;
    this.ruleUsageText = "";
    this.matcherCondition = { kind: "exact", value: "" };
    this.matchTestResult = null;
    this.matcherTestError = "";
    this.matchExamples = [];
    this.responseBodyFileDetails = "";
    this.responseBodyFileError = "";
    this.fileChoiceBusy = false;
    this.bodyFileTouched = false;
    this.editBodyDisabled = true;
    this.savedNonBodySnapshot = "";
    this.savedActivationRule = null;
    this.responseCanSave = true;
    this.compressionType = "";
    this.compressionUnavailableText = "";
    this.savedHeaderText = "";
    this.originalResponseHeaders = [];
    this.bodyMode = "text";
    this.bodyByteSummary = "";
    this.bodyTextEncoding = "utf-8";
    this.bodyOriginalBytes = null;
    this.bodyViewBytes = null;
    this.bodyViewValue = "";
    this.capturedDraft = null;
    this.capturedOrigin = null;
    window.clearTimeout(this.matcherTimer);
    this.matcherTestRevision++;
    this.matcherTestBusy = false;
    this.autoResponseEnabled.checked = true;
    for (const field of ["requestHeaders", "responseHeaders"]) {
      (this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement).setCustomValidity(
        "",
      );
    }
    this.responseOpen = true;
    this.sourceSessionId = "";
    this.sourceLabel = "";
    this.sourceAvailable = false;
    this.sourceAvailabilityText = "";
    this.bodyStatusText = "";
    this.bodyHidden = false;
    this.encodingOptionsHidden = true;
    this.autoResponseError = "";
    this.requestHeadersError = "";
    this.responseHeadersError = "";
    this.sourceQuery = null;
  }
  private openEditor(focusProperties = true): void {
    this.editorHidden = false;
    this.$flushUpdates();
    if (!this.switchingCollection && !this.stagingCollection) {
      this.autoResponseForm.scrollIntoView({ block: "start" });
    }
    if (focusProperties) {
      (this.autoResponseForm.elements.namedItem("name") as HTMLInputElement).focus({
        preventScroll: true,
      });
    }
  }
  async refreshSourceAvailability(): Promise<void> {
    const id = this.sourceSessionId;
    if (!id || this.editorHidden || !this.isConnected) {
      return;
    }
    if (this.sourceQuery) {
      return this.sourceQuery;
    }
    const revision = this.editorRevision;
    const pending = (async () => {
      try {
        const detail = await invoke<SessionDetail>("session_detail", { id });
        if (revision !== this.editorRevision || !this.isConnected) {
          return;
        }
        const request = detail.requests.find((head) => head.boundary === "client-request");
        this.sourceLabel = request?.target
          ? `${request.method ?? "Request"} ${shortUrl(request.target)}`
          : "Original captured request";
        this.sourceAvailable = true;
        this.sourceAvailabilityText = "";
      } catch (error: unknown) {
        if (revision !== this.editorRevision || !this.isConnected) {
          return;
        }
        this.sourceAvailable = false;
        this.sourceAvailabilityText = /evict|unavailable|not found/i.test(describeError(error))
          ? "Source no longer in Traffic."
          : "Source could not be checked. Try again when Traffic is available.";
      }
    })();
    this.sourceQuery = pending;
    try {
      await pending;
    } finally {
      if (this.sourceQuery === pending) {
        this.sourceQuery = null;
      }
    }
  }
  async showSourceTraffic(event: Event): Promise<void> {
    event.preventDefault();
    const id = this.sourceSessionId;
    const revision = this.editorRevision;
    await this.refreshSourceAvailability();
    if (this.sourceAvailable && revision === this.editorRevision && id === this.sourceSessionId) {
      this.$emit("source-traffic-request", id);
    }
  }
  clearHeaderError(field: "requestHeaders" | "responseHeaders"): void {
    (this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement).setCustomValidity("");
    if (field === "requestHeaders") {
      this.requestHeadersError = "";
    } else {
      this.responseHeadersError = "";
    }
    this.autoResponseError = "";
    if (field === "requestHeaders") {
      this.onMatcherChange();
    }
  }
  private parseEditorHeaders(
    field: "requestHeaders" | "responseHeaders",
    data: FormData,
  ): ReturnType<typeof parseHeaderLines> {
    try {
      return parseHeaderLines(String(data.get(field) ?? ""));
    } catch (error: unknown) {
      const message = describeError(error);
      if (field === "requestHeaders") {
        this.requestHeadersError = message;
      } else {
        this.responseHeadersError = message;
      }
      const input = this.autoResponseForm.elements.namedItem(field) as HTMLTextAreaElement;
      if (field === "responseHeaders") {
        this.responseOpen = true;
      } else {
        this.requestHeadersOpen = true;
      }
      this.$flushUpdates();
      input.setCustomValidity(message);
      input.focus();
      throw error;
    }
  }
  private renderAutoResponseRules(status: AutomationStatus): void {
    const rules = this.autoResponseRules(status);
    const owned = experimentRuleIds(status);
    const retained = new Set(
      status.rules.filter((rule) => !owned.has(rule.id)).map((rule) => rule.id),
    );
    this.ruleSelection.ids = new Set([...this.ruleSelection.ids].filter((id) => retained.has(id)));
    if (this.ruleSelection.focused && !retained.has(this.ruleSelection.focused)) {
      this.ruleSelection.focused = null;
    }
    if (this.ruleSelection.anchor && !retained.has(this.ruleSelection.anchor)) {
      this.ruleSelection.anchor = null;
    }
    this.rules = rules.map((rule, index) => {
      const headers = rule.matcher.requestHeaders.length;
      const enabled = rule.enabled ?? true;
      const diagnostic = status.diagnostics?.find((item) => item.ruleId === rule.id);
      const earlier = status.rules.find((item) => item.id === diagnostic?.supersededBy);
      const asset = this.assetsIndex.get(rule.request.responseAsset ?? "");
      return {
        ...rule,
        enabled,
        order: index + 1,
        name: rule.displayName ?? "Saved response",
        criteria: `${rule.matcher.method ?? "ANY"} ${rule.matcher.url?.kind === "exact" ? rule.matcher.url.value : rule.matcher.url?.kind === "pattern" ? rule.matcher.url.value.address : rule.matcher.url?.kind === "regex" ? rule.matcher.url.value.pattern : "any URL"}${headers === 0 ? "" : ` · ${headers} header condition${headers === 1 ? "" : "s"}`} · saved response`,
        state: enabled
          ? status.autoresponsesEnabled === false
            ? "Enabled · Paused"
            : "Enabled"
          : "Disabled",
        toggleLabel: enabled ? "Disable" : "Enable",
        first: index === 0,
        last: index === rules.length - 1,
        selected: this.ruleSelection.ids.has(rule.id),
        selectionState: String(this.ruleSelection.ids.has(rule.id)),
        tabIndex: this.ruleSelection.focused === rule.id ? "0" : "-1",
        shadowedName: earlier?.displayName ?? earlier?.id ?? "",
        shadowedBy: diagnostic?.supersededBy ?? "",
        diagnostic: diagnostic ? (diagnostic.duplicateResponse ? "Duplicate" : "Shadowed") : "",
        responseText: asset?.linkedBody
          ? `${asset.status} · File`
          : asset
            ? `${asset.status} · ${formatBytes(asset.bodyBytes)}`
            : "Saved response",
      };
    });
    this.renderRuleSelection();
    const selected = this.rules.find((rule) => rule.id === this.editingAutoResponseId);
    this.ruleSupersededBy = selected?.shadowedBy ?? "";
    this.ruleDiagnosticText = selected?.shadowedBy
      ? `${selected.enabled === false ? "If enabled, this" : "This"} rule is always superseded by “${selected.shadowedName}”, which has equivalent matching conditions${selected.diagnostic === "Duplicate" ? " and the same response" : ""}.`
      : "";
    if (this.existingResponse) {
      this.renderRuleUsage(status);
    }
  }
  private renderRuleUsage(status: AutomationStatus): void {
    const usage = status.usage?.find((item) => item.ruleId === this.editingAutoResponseId);
    this.ruleUsageText = usage
      ? `${usage.matches} retained match${usage.matches === 1 ? "" : "es"} · Last matched ${new Date(usage.lastMatchedAt).toLocaleString()}`
      : "No matches in retained Traffic.";
  }
  ruleClick(rule: AutomationRule, event: MouseEvent): void {
    if (!this.ignoreRuleClick) {
      void this.selectRule(rule, event);
    }
  }
  beginRulePointer(id: string, event: PointerEvent): void {
    if (
      event.button !== 0 ||
      !event.isPrimary ||
      this.savingAutoResponse ||
      this.changingAutoResponse
    ) {
      return;
    }
    const target = event.target as HTMLElement;
    if (
      target.closest("input,button,a,select,textarea") ||
      (event.pointerType === "touch" && !target.closest(".rule-drag-grip"))
    ) {
      return;
    }
    event.preventDefault();
    this.finishRuleDrag();
    this.rulePointer = { id, event, x: event.clientX, y: event.clientY };
    this.autoResponseList.setPointerCapture(event.pointerId);
    (event.currentTarget as HTMLElement).focus({ preventScroll: true });
  }
  moveRulePointer(event: PointerEvent): void {
    const pointer = this.rulePointer;
    if (!pointer || pointer.event.pointerId !== event.pointerId) {
      return;
    }
    if (!this.draggedRuleId) {
      if (Math.hypot(event.clientX - pointer.x, event.clientY - pointer.y) < 6) {
        return;
      }
      if (!this.startRuleDrag(pointer.id)) {
        return;
      }
    }
    event.preventDefault();
    this.dragPointer = { x: event.clientX, y: event.clientY };
    this.ruleDragX = event.clientX + 12 + "px";
    this.ruleDragY = event.clientY + 12 + "px";
    this.updateRuleDrop();
    if (this.dragScrollTimer === undefined) {
      this.dragScrollTimer = window.setInterval(() => this.scrollRuleDrag(), 30);
    }
  }
  async endRulePointer(event: PointerEvent): Promise<void> {
    const pointer = this.rulePointer;
    if (!pointer || pointer.event.pointerId !== event.pointerId) {
      return;
    }
    this.ignoreRuleClick = true;
    window.setTimeout(() => {
      this.ignoreRuleClick = false;
    }, 0);
    if (this.draggedRuleId) {
      await this.dropRule(event);
    } else {
      const rule = this.rules.find((rule) => rule.id === pointer.id);
      this.finishRuleDrag();
      if (rule) {
        await this.selectRule(rule, pointer.event);
      }
    }
  }
  private startRuleDrag(id: string): boolean {
    if (this.hasNativeSelection) {
      this.ruleDragText =
        "Select only saved responses to drag their priority. Other rule order is available in properties.";
      return false;
    }
    if (this.savingAutoResponse || this.changingAutoResponse) {
      return false;
    }
    const originalSelection = this.ruleSelection.clone();
    if (!this.ruleSelection.ids.has(id)) {
      if (this.draftDirty) {
        this.finishRuleDrag();
        this.ruleDragText = "Save or revert your properties before dragging another rule.";
        return false;
      }
      this.ruleSelection.replace(id);
      this.renderRuleSelection();
    }
    this.dragOriginalSelection = originalSelection;
    this.ruleSelection.focused = id;
    this.renderRuleSelection();
    this.draggedRuleId = id;
    this.draggedRuleIds = this.rules
      .filter((rule) => this.ruleSelection.ids.has(rule.id))
      .map((rule) => rule.id);
    this.dragGeneration = this.automationStatus?.generation ?? null;
    this.ruleDragging = true;
    this.ruleDragText = `Moving ${this.draggedRuleIds.length} rule${this.draggedRuleIds.length === 1 ? "" : "s"} · Hold at an edge to scroll.`;
    this.ruleDragLabel =
      this.draggedRuleIds.length === 1
        ? (this.rules.find((rule) => rule.id === id)?.name ?? "Saved response")
        : `Moving ${this.draggedRuleIds.length} rules`;
    return true;
  }
  private updateRuleDrop(): void {
    this.ruleDropId = "";
    const pointer = this.dragPointer;
    if (!pointer) {
      return;
    }
    const list = this.autoResponseList.getBoundingClientRect();
    if (
      pointer.x < list.left ||
      pointer.x > list.right ||
      pointer.y < list.top ||
      pointer.y > list.bottom
    ) {
      return;
    }
    const rows = Array.from(this.autoResponseList.querySelectorAll<HTMLElement>("[data-rule-id]"));
    const row = rows.find((row) => row.getBoundingClientRect().bottom > pointer.y) ?? rows.at(-1);
    if (!row || this.draggedRuleIds.includes(row.dataset.ruleId ?? "")) {
      return;
    }
    const rect = row.getBoundingClientRect();
    this.ruleDropId = row.dataset.ruleId ?? "";
    this.ruleDropAfter = pointer.y >= rect.top + rect.height / 2;
  }
  private scrollRuleDrag(): void {
    const pointer = this.dragPointer;
    if (!pointer || !this.draggedRuleId || !this.isConnected || this.view !== "automation") {
      this.finishRuleDrag();
      return;
    }
    const list = this.autoResponseList,
      rect = list.getBoundingClientRect(),
      edge = Math.min(48, rect.height / 4);
    if (
      pointer.x < rect.left ||
      pointer.x > rect.right ||
      pointer.y < rect.top ||
      pointer.y > rect.bottom
    ) {
      return;
    }
    const direction =
      pointer.y < rect.top + edge
        ? -(1 - (pointer.y - rect.top) / edge)
        : pointer.y > rect.bottom - edge
          ? 1 - (rect.bottom - pointer.y) / edge
          : 0;
    if (direction) {
      list.scrollTop += direction * 18;
      this.updateRuleDrop();
    }
  }
  finishRuleDrag(): void {
    const pointer = this.rulePointer;
    this.rulePointer = null;
    if (pointer && this.autoResponseList.hasPointerCapture(pointer.event.pointerId)) {
      this.autoResponseList.releasePointerCapture(pointer.event.pointerId);
    }
    window.clearInterval(this.dragScrollTimer);
    this.dragScrollTimer = undefined;
    this.dragPointer = null;
    this.draggedRuleId = null;
    this.draggedRuleIds = [];
    this.dragGeneration = null;
    this.ruleDropId = "";
    this.ruleDropAfter = false;
    this.ruleDragging = false;
    this.ruleDragText = "";
    if (this.dragOriginalSelection) {
      this.ruleSelection = this.dragOriginalSelection;
      this.dragOriginalSelection = null;
      this.renderRuleSelection();
    }
  }
  lostRulePointer(event: PointerEvent): void {
    if (
      this.rulePointer?.event.pointerId === event.pointerId &&
      !this.autoResponseList.hasPointerCapture(event.pointerId)
    ) {
      this.finishRuleDrag();
    }
  }
  cancelRulePointer(): void {
    const focused = this.dragOriginalSelection?.focused;
    this.finishRuleDrag();
    if (focused && this.isConnected && this.view === "automation") {
      this.focusRule(focused);
    }
  }
  private async dropRule(event: PointerEvent): Promise<void> {
    event.preventDefault();
    this.dragPointer = { x: event.clientX, y: event.clientY };
    this.updateRuleDrop();
    const dragged = this.draggedRuleId,
      selected = this.draggedRuleIds,
      id = this.ruleDropId,
      after = this.ruleDropAfter,
      generation = this.dragGeneration,
      selection = this.ruleSelection.clone();
    this.finishRuleDrag();
    if (this.savingAutoResponse || this.changingAutoResponse) {
      return;
    }
    const current = this.automationStatus;
    if (!dragged || !id || current === null) {
      return;
    }
    if (current.generation !== generation) {
      this.automationText = "Rules changed while dragging. Try the move again.";
      this.automationKind = "error";
      return;
    }
    const before = this.autoResponseRules(current);
    const group = before.filter((rule) => selected.includes(rule.id));
    const ordered = before.filter((rule) => !selected.includes(rule.id));
    const to = ordered.findIndex((rule) => rule.id === id);
    if (!group.length || to < 0) {
      return;
    }
    ordered.splice(to + (after ? 1 : 0), 0, ...group);
    if (ordered.every((rule, index) => rule.id === before[index]?.id)) {
      return;
    }
    this.changingAutoResponse = true;
    try {
      const status = await this.activateAutoResponseOrder(current, ordered);
      this.ruleSelection = selection;
      this.rememberRuleChange(before, this.autoResponseRules(status), "Changed rule priority.");
      this.renderAutomation(status);
      this.changingAutoResponse = false;
      if (
        !this.draftDirty &&
        this.ruleSelection.ids.size === 1 &&
        this.editingAutoResponseId !== dragged
      ) {
        const rule = status.rules.find((rule) => rule.id === dragged);
        if (rule) {
          await this.editAutoResponse(rule, true);
        }
      }
      if (
        !this.rulePointer &&
        this.ruleSelection.focused === dragged &&
        this.isConnected &&
        this.view === "automation"
      ) {
        this.focusRule(dragged);
      }
    } catch (error: unknown) {
      this.automationText = `Saved response reorder failed: ${describeError(error)}`;
      this.automationKind = "error";
    } finally {
      this.changingAutoResponse = false;
    }
  }
  async showMatchedRule(
    id: string,
    isCurrent: () => boolean = () => true,
    confirmLeave: () => Promise<boolean> = () => this.canLeaveEditor(),
  ): Promise<void> {
    const originRevision = this.editorRevision,
      originDirty = this.draftDirty;
    try {
      if (!isCurrent()) {
        return;
      }
      const owner = this.automationStatus?.responseSets?.find((set) =>
        set.rules.some((rule) => rule.id === id),
      );
      if (owner) {
        if (!(await confirmLeave()) || !isCurrent()) {
          return;
        }
        this.editorHidden = true;
        this.automationSection = "experiments";
        this.$flushUpdates();
        await this.openResponseSet(new CustomEvent("set-open", { detail: { id: owner.id } }), true);
        return;
      }
      let rule = this.automationStatus?.rules.find((rule) => rule.id === id);
      if (!rule) {
        this.showNotice(
          "Historical rule",
          "The response records which rule won, but that rule is no longer in the active list.",
          null,
          null,
        );
        return;
      }
      if (!(await confirmLeave()) || !isCurrent()) {
        return;
      }
      rule = this.automationStatus?.rules.find((rule) => rule.id === id);
      if (!rule) {
        this.showNotice(
          "Historical rule",
          "The response records which rule won, but that rule is no longer in the active list.",
          null,
          null,
        );
        return;
      }
      this.ruleSearch = "";
      this.ruleFilter = "all";
      this.ruleSelection.replace(id);
      this.renderRuleSelection();
      if (!originDirty && rule.id === this.editingAutoResponseId && !this.editorHidden) {
        this.showAutomationSection(this.responseExperiment ? "experiments" : "responses");
      } else {
        const pending = this.editAutoResponse(rule, true),
          revision = this.editorRevision;
        await pending;
        if (revision !== this.editorRevision) {
          return;
        }
      }
      if (
        !isCurrent() ||
        !this.isConnected ||
        this.view !== "automation" ||
        this.editingAutoResponseId !== id ||
        this.ruleSelection.ids.size !== 1 ||
        !this.ruleSelection.ids.has(id)
      ) {
        return;
      }
      this.highlightedRuleId = id;
      this.$flushUpdates();
      const card = this.getRootNode() as ShadowRoot;
      const element = card.getElementById(`rule-${id}`);
      if (!element) {
        this.showNotice(
          "Historical rule",
          "The response records which rule won, but that rule is no longer in the active list.",
          null,
          null,
        );
        return;
      }
      element.scrollIntoView({ block: "center" });
      element.focus({ preventScroll: true });
    } finally {
      // A confirmed discard is consumed only by replacing/leaving that editor.
      // An aborted reveal must not leave unchanged typed fields marked clean.
      if (originDirty && originRevision === this.editorRevision && !this.draftDirty) {
        this.refreshResponseDirty();
      }
    }
  }
  async refreshAutomation(): Promise<void> {
    try {
      const [status, assets] = await Promise.all([
        invoke<AutomationStatus>("automation_status"),
        invoke<ResponseAsset[]>("response_assets"),
      ]);
      this.assetsIndex = new Map(assets.map((asset) => [`${asset.id}@${asset.revision}`, asset]));
      this.renderAutomation(status);
    } catch (error: unknown) {
      this.automationText = `Automation query failed: ${describeError(error)}`;
    }
  }

  async beginCapturedResponseFlow(): Promise<void> {
    if (this.savingAutoResponse || this.changingAutoResponse) {
      return;
    }
    if (this.selection?.reusable) {
      await this.beginAutoResponseFromSelected();
    } else {
      this.activateView("traffic");
    }
  }

  async beginAutoResponseFromSelected(): Promise<void> {
    if (this.savingAutoResponse) {
      return;
    }
    if (this.selection === null) {
      this.showNotice(
        "Select a completed response",
        "Choose a request in Traffic whose client response body was retained, then try again.",
        null,
        null,
      );
      return;
    }
    await this.populateCapturedAutoResponse(this.selection.sessionId, this.selection.detail);
  }

  async populateCapturedAutoResponse(
    sessionId: string,
    detail: SessionDetail,
    restored?: CapturedDraft,
    experiment = false,
    variant?: ExperimentSelection,
    collectionOperation = false,
  ): Promise<void> {
    if (this.savingAutoResponse) {
      return;
    }
    const source = clientResponseSource(detail);
    if (
      source === null ||
      source.request.method === null ||
      source.request.target === null ||
      source.response.status === null
    ) {
      this.showNotice(
        "Response cannot be reused",
        autoResponseUnavailableReason(detail),
        null,
        null,
      );
      return;
    }
    if (experiment && !variant) {
      if (!(await this.canLeaveEditor())) {
        return;
      }
      this.automationSection = "experiments";
      this.responseSets.showCreate({ sessionId, detail, reusable: true });
      return;
    }
    if ((!collectionOperation && !(await this.canLeaveEditor())) || this.savingAutoResponse) {
      return;
    }
    this.editingAutoResponseId = null;
    this.prepareEditor(experiment);
    this.editingVariant = variant ?? null;
    this.autoResponseEnabled.checked = !experiment;
    this.capturedOrigin = { sessionId, detail: structuredClone(detail) };
    const elements = this.autoResponseForm.elements;
    const url = implicitDefaultPort(source.request.target);
    (elements.namedItem("name") as HTMLInputElement).value =
      `${source.request.method} ${shortUrl(url)}`;
    this.setEditorMethod(source.request.method.toUpperCase());
    (elements.namedItem("url") as HTMLInputElement).value = url;
    this.matcherCondition = { kind: "exact", value: url };
    this.matchTestUrl.value = url;
    this.matchTestMethod.value = source.request.method;
    this.matchTestHeaders.value = source.request.headers
      .filter((header) => !header.sensitive && !header.binary)
      .map((header) => header.name + ": " + header.value)
      .join("\n");
    this.autoResponseStatus.value = String(source.response.status);
    this.responseReadOnly = false;
    this.autoResponseMediaType.value = source.body.mediaType ?? "";
    this.responseFieldsHidden = false;
    (elements.namedItem("responseHeaders") as HTMLTextAreaElement).value = "";
    this.responseHeadersHidden = false;
    this.capturedOptionsHidden = false;
    this.autoResponseEditBody.checked = false;
    this.autoResponsePreserveEncoding.checked = true;
    this.preserveEncodingDisabled = source.body.contentCodings.length === 0;
    this.encodingOptionsHidden = source.body.contentCodings.length === 0;
    this.autoResponseBody.value = "";
    this.bodyReadOnly = true;
    this.editorTitle = experiment
      ? variant
        ? "Add response"
        : "New response set"
      : "New rule from captured response";
    this.renderExperimentState();
    this.sourceText = `${source.response.status} · ${formatBytes(source.body.retainedBytes)} · ${source.body.mediaType ?? "Unknown content type"}${source.body.contentCodings.length === 0 ? "" : ` · ${source.body.contentCodings.join(", ")}`}`;
    this.sourceSessionId = sessionId;
    this.sourceLabel = `${source.request.method} ${shortUrl(source.request.target)}`;
    this.sourceAvailabilityText = "Checking source in Traffic…";
    const captured: AutoResponseSource = {
      kind: "captured",
      sessionId,
      boundary: "client-response",
      contentCodings: source.body.contentCodings,
      bodyEditable: false,
      textEncoding: editableCharacterEncoding(source.body.charset),
    };
    this.autoResponseSourceState = captured;
    const revision = this.editorRevision;
    this.updateAutoResponseHeaderFilterState();
    this.updateAutoResponseBodyEditState();
    this.openEditor();
    if (experiment) {
      this.draftDirty = true;
    }
    const capturedIdentity = JSON.parse(this.nonBodySnapshot()) as Record<string, unknown>;
    void this.refreshSourceAvailability();

    this.loadingSavedResponse = true;
    this.responseReadOnly = true;
    try {
      const headers =
        restored?.headers ??
        (await invoke<ResponseAsset["headers"]>("autoresponse_source_headers", { id: sessionId }));
      if (revision !== this.editorRevision || !this.isConnected) {
        return;
      }
      this.savedHeaderText = this.editableHeaders({ headers });
      (elements.namedItem("responseHeaders") as HTMLTextAreaElement).value = this.savedHeaderText;
      this.capturedDraft = {
        sessionId,
        detail: structuredClone(detail),
        headers,
        inspection: restored?.inspection ?? null,
      };
      this.responseReadOnly = false;
      const fields = JSON.parse(this.nonBodySnapshot()) as Record<string, unknown>;
      for (const key of ["name", "method", "url", "requestHeaders", "retainAdvanced", "examples"]) {
        fields[key] = capturedIdentity[key];
      }
      this.responseBaseline.capture({
        ...(this.responseValues() as object),
        fields: JSON.stringify(fields),
      });
      this.refreshResponseDirty();
    } catch (error: unknown) {
      if (revision === this.editorRevision) {
        this.autoResponseError = describeError(error);
        this.responseReadOnly = true;
      }
    } finally {
      if (revision === this.editorRevision) {
        this.loadingSavedResponse = false;
      }
    }
    if (revision !== this.editorRevision || !this.isConnected) {
      return;
    }

    if (source.body.retainedBytes === 0) {
      captured.bodyEditable = true;
      this.setBodyContent([], captured.textEncoding);
      this.bodyStatusText = "Empty response body. Enable editing to replace it.";
      this.updateAutoResponseBodyEditState();
      this.refreshResponseDirty();
      return;
    }
    if (source.body.retainedBytes > MAX_AUTORESPONSE_EDIT_BYTES) {
      await this.showCapturedBodyPreview(
        sessionId,
        revision,
        "Body exceeds the 16 MiB editor limit.",
      );
      return;
    }
    this.bodyStatusText = "Loading response body…";
    try {
      const inspection =
        restored?.inspection ??
        (await invoke<BodyInspection>("inspect_body", {
          request: {
            sessionId,
            boundary: "client-response",
            representation: "editable",
            decodeContent: true,
            offset: 0,
            maxBytes: MAX_AUTORESPONSE_EDIT_BYTES,
          },
        }));
      if (
        revision !== this.editorRevision ||
        this.autoResponseSourceState !== captured ||
        !this.isConnected
      ) {
        return;
      }
      if (this.capturedDraft) {
        this.capturedDraft.inspection = inspection;
      }
      if (
        !inspection.truncated &&
        inspection.metadata.availability === "complete" &&
        (inspection.bytesBase64 != null ||
          (inspection.representation === "original-text" && inspection.textEncoding !== null))
      ) {
        const bytes =
          inspection.bytesBase64 != null
            ? decodeBodyBase64(inspection.bytesBase64)
            : encodeBody(inspection.display, "text", inspection.textEncoding);
        this.setBodyContent(bytes, inspection.textEncoding, inspection.display);
        captured.bodyEditable = true;
        captured.textEncoding = inspection.textEncoding;
        this.updateAutoResponseBodyEditState();
      } else {
        await this.showCapturedBodyPreview(
          sessionId,
          revision,
          inspection.truncated
            ? "The body preview is incomplete and cannot be saved as a replacement."
            : "A complete editable body is unavailable.",
        );
      }
    } catch (error: unknown) {
      if (revision === this.editorRevision && this.autoResponseSourceState === captured) {
        await this.showCapturedBodyPreview(
          sessionId,
          revision,
          "Decoded body unavailable: " + describeError(error) + ".",
        );
      }
    }
    if (revision === this.editorRevision) {
      this.refreshResponseDirty();
    }
  }

  private async showCapturedBodyPreview(
    sessionId: string,
    revision: number,
    reason: string,
  ): Promise<void> {
    if (revision !== this.editorRevision || !this.isConnected) {
      return;
    }
    this.bodyStatusText = reason + " Serve the complete body from a file to replace it.";
    try {
      const preview = await invoke<BodyInspection>("inspect_body", {
        request: {
          sessionId,
          boundary: "client-response",
          representation: "bytes",
          decodeContent: false,
          offset: 0,
          maxBytes: 4096,
        },
      });
      if (revision !== this.editorRevision || !this.isConnected) {
        return;
      }
      this.bodyMode = "hex";
      this.autoResponseBody.value = preview.display;
      this.bodyReadOnly = true;
      this.editBodyDisabled = true;
      this.bodyByteSummary = `Hex · First ${formatBytes(preview.displayBytes)} of ${formatBytes(preview.metadata.retainedBytes)} retained bytes · Before decoding compression`;
      this.bodyStatusText =
        reason +
        " This bounded preview is read-only. Serve the complete body from a file to replace it.";
    } catch {
      if (revision === this.editorRevision) {
        this.bodyStatusText =
          reason +
          " Original-byte preview is unavailable. Serve the complete body from a file to replace it.";
      }
    }
    this.refreshResponseDirty();
  }

  async beginScratchAutoResponse(collectionOperation = false): Promise<void> {
    if (this.savingAutoResponse) {
      return;
    }
    if ((!collectionOperation && !(await this.canLeaveEditor())) || this.savingAutoResponse) {
      return;
    }
    this.prepareEditor(collectionOperation);
    this.editingAutoResponseId = null;
    this.autoResponseSourceState = { kind: "scratch" };
    this.editorTitle = "New response from scratch";
    this.sourceText =
      "Custom response. Matching requests receive the status, headers, and body below.";
    this.responseReadOnly = false;
    this.responseFieldsHidden = false;
    this.responseHeadersHidden = false;
    this.capturedOptionsHidden = true;
    this.bodyReadOnly = false;
    this.editBodyDisabled = false;
    this.setBodyContent([], "utf-8");
    this.captureResponseBaseline();
    this.updateAutoResponseHeaderFilterState();
    this.openEditor();
  }

  async cancelAutoResponseEdit(): Promise<void> {
    if (
      this.collectionAdding ||
      this.loadingSavedResponse ||
      this.fileChoiceBusy ||
      this.savingAutoResponse ||
      this.responseSets.busy ||
      this.resolvingUnsaved
    ) {
      return;
    }
    if (this.collectionOpen) {
      this.captureCollectionResponse();
      this.editorRevision++;
      this.editorHidden = true;
      this.collectionRuleId = "";
      this.responseSets.select("");
      this.$flushUpdates();
      this.responseSets.addButton.focus();
      return;
    }
    if (this.savingAutoResponse) {
      return;
    }
    if (!(await this.canLeaveEditor())) {
      return;
    }
    this.editorRevision++;
    this.editorHidden = true;
    this.autoResponseSourceState = null;
    this.editingAutoResponseId = null;
    this.ruleSelection.clear();
    this.renderRuleSelection();
    this.draftDirty = false;
    this.$flushUpdates();
    if (this.editorOpener?.isConnected && this.editorOpener.getClientRects().length) {
      this.editorOpener.focus();
    } else if (this.responseExperiment) {
      this.responseSets.addButton.focus();
    } else {
      this.newRuleButton.focus();
    }
  }

  updateAutoResponseHeaderFilterState(): void {
    this.requestHeadersHidden = false;
    this.requestHeadersOpen =
      this.autoResponseMethod.value === "POST" ||
      Boolean(
        (this.autoResponseForm.elements.namedItem("requestHeaders") as HTMLTextAreaElement).value,
      );
    this.customMethodHidden = this.autoResponseMethod.value !== "CUSTOM";
  }

  updateAutoResponseBodyEditState(): void {
    const source = this.autoResponseSourceState;
    const editable = source?.kind === "captured" && source.bodyEditable;
    if (!editable) {
      this.autoResponseEditBody.checked = false;
    }
    this.editBodyDisabled = !editable;
    this.bodyReadOnly = !this.autoResponseEditBody.checked;
    this.updateCompressionChoice();
    if (editable) {
      this.bodyStatusText = this.autoResponseEditBody.checked
        ? "Edits replace the captured body. Original traffic keeps its original bytes."
        : "Complete body preview. Enable editing or open the dialog to replace it.";
    }
  }

  private updateCompressionChoice(): void {
    const source = this.autoResponseSourceState;
    const codings =
      source?.kind === "captured"
        ? source.contentCodings
        : (this.savedResponse?.contentCodings ?? []);
    this.compressionType = formatContentCodings(codings);
    this.encodingOptionsHidden = codings.length === 0;
    const editable =
      source?.kind === "captured" ? this.autoResponseEditBody.checked : !this.bodyReadOnly;
    const unsupported = !supportedContentCodings(codings);
    this.compressionUnavailableText = unsupported
      ? "Original compression is unsupported. Replacement bodies will be served without compression."
      : "";
    if (unsupported && (this.responseBodyPath || editable)) {
      this.autoResponsePreserveEncoding.checked = false;
    }
    this.preserveEncodingDisabled =
      codings.length === 0 || unsupported || (!this.responseBodyPath && !editable);
  }

  async saveAutoResponse(event: Event): Promise<boolean> {
    event.preventDefault();
    if (this.collectionOpen && this.responseSets.tourStep > 0) {
      return false;
    }
    if (this.collectionOpen && this.responseExperiment && !this.stagingCollection) {
      return this.saveCollection();
    }
    if (this.savingAutoResponse || this.changingAutoResponse || this.fileChoiceBusy) {
      return false;
    }
    if (this.variantHistorical) {
      this.autoResponseError = "Duplicate an earlier revision in Response sets before editing it.";
      return false;
    }
    if (this.autoResponseSourceState?.kind === "captured" && !this.capturedDraft) {
      this.autoResponseError =
        "Original response headers could not be loaded. Choose another response in Traffic.";
      return false;
    }
    if (this.editingAutoResponseId && !this.savedResponse) {
      this.autoResponseError = "Wait for the saved response to load before saving.";
      return false;
    }
    this.refreshResponseDirty();
    if (this.existingResponse && !this.draftDirty) {
      return true;
    }
    const data = new FormData(this.autoResponseForm);
    const source = this.autoResponseSourceState;
    if (source === null) {
      return false;
    }
    const editingId = this.editingAutoResponseId,
      experiment = this.responseExperiment,
      variantSelection = this.editingVariant ? { ...this.editingVariant } : null;
    const replaceBody = this.autoResponseEditBody.checked && !this.responseBodyPath;
    const preserveEncoding = this.autoResponsePreserveEncoding.checked;
    this.savingAutoResponse = true;
    this.autoResponseError = "";
    this.requestHeadersError = "";
    this.responseHeadersError = "";
    if (!this.stagingCollection) {
      this.automationText = "Validating and saving the saved response…";
      this.automationKind = "progress";
    }
    try {
      const displayName = String(data.get("name") ?? "").trim();
      if (!displayName) {
        throw new Error(`Enter a ${this.responseNameLabel.toLowerCase()}.`);
      }
      const editableResponseHeaders = this.parseEditorHeaders("responseHeaders", data).filter(
        (header) =>
          !["content-length", "content-encoding", "content-type"].includes(
            header.name.toLowerCase(),
          ),
      );
      const responseHeaders = this.mergeResponseHeaders(
        String(data.get("responseHeaders") ?? ""),
        editableResponseHeaders,
      );
      const capturedBytes =
        source.kind === "captured" && replaceBody ? this.currentBodyBytes() : null;
      const editedBody =
        capturedBytes !== null &&
        (!preserveEncoding ||
          this.bodyOriginalBytes === null ||
          !equalBodyBytes(capturedBytes, this.bodyOriginalBytes))
          ? capturedBytes
          : null;
      const current = await invoke<AutomationStatus>("automation_status");
      const variantRevision = variantSelection
        ? current.responseSets?.find((set) => set.id === variantSelection.experimentId)
        : undefined;
      const existing =
        editingId === null
          ? undefined
          : (variantRevision?.rules ?? current.rules).find(
              (candidate) => candidate.id === editingId,
            );
      if (editingId !== null && existing === undefined) {
        throw new Error("This rule was removed. Cancel editing and create a new rule.");
      }
      const template = existing ?? this.responseRuleTemplate ?? undefined;
      const matcher = this.currentMatcher(data, template);
      await invoke<MatchTestResult>("test_autoresponse_match", {
        input: {
          matcher,
          method: matcher.method ?? "GET",
          url: "https://example.test/",
          headers: [],
          ruleId: editingId,
          enabled: this.autoResponseEnabled.checked,
        },
      });
      let assetReference: string;
      if (source.kind === "existing") {
        assetReference = source.assetReference;
        const saved = this.savedResponse;
        if (!saved) {
          throw new Error("Wait for the saved response to load before saving.");
        }
        const bodyBytes =
          this.editBodyDisabled || this.responseBodyPath ? null : this.currentBodyBytes();
        const bodyChanged =
          (bodyBytes !== null &&
            (this.bodyOriginalBytes === null ||
              !equalBodyBytes(bodyBytes, this.bodyOriginalBytes))) ||
          (this.bodyFileTouched && !this.responseBodyPath) ||
          (!preserveEncoding && saved.contentCodings.length > 0 && bodyBytes !== null);
        const compressionChanged =
          Boolean(saved.asset.linkedBody) &&
          Boolean(saved.asset.linkedBody?.applyOriginalCompression) !== preserveEncoding;
        if (
          bodyChanged ||
          this.bodyFileTouched ||
          compressionChanged ||
          String(data.get("responseHeaders") ?? "") !== this.savedHeaderText ||
          Number(data.get("status")) !== saved.asset.status ||
          String(data.get("mediaType") ?? "") !== (saved.asset.mediaType ?? "")
        ) {
          if (
            bodyChanged &&
            !this.responseBodyPath &&
            saved.asset.linkedBody &&
            this.bodyMode === "text" &&
            this.bodyTextEncoding === null
          ) {
            throw new Error(
              "Content type has an unsupported charset. Choose UTF-8, edit in Hex, or serve the response body from a file.",
            );
          }
          const asset = await invoke<ResponseAsset>("edit_response_asset", {
            input: {
              assetReference,
              status: Number(data.get("status")),
              headers: responseHeaders,
              mediaType: optionalText(data.get("mediaType")),
              decodedBody: bodyChanged && !this.responseBodyPath ? bodyBytes : null,
              bodyPath: this.bodyFileTouched ? this.responseBodyPath : null,
              bodyFileMode: this.responseBodyFileMode,
              preserveContentEncoding: preserveEncoding,
            },
          });
          assetReference = `${asset.id}@${asset.revision}`;
        }
      } else {
        const suffix = crypto.randomUUID().replaceAll("-", "");
        const assetId = `autoresponse-${suffix}`;
        let asset =
          source.kind === "captured"
            ? await invoke<ResponseAsset>("create_response_asset_from_session", {
                input: {
                  id: assetId,
                  revision: 1,
                  exchangeId: source.sessionId,
                  boundary: source.boundary,
                  decodedBody: editedBody,
                  bodyPath: this.responseBodyPath,
                  bodyFileMode: this.responseBodyFileMode,
                  preserveContentEncoding: preserveEncoding,
                },
              })
            : this.responseBodyPath
              ? await invoke<ResponseAsset>("import_response_asset", {
                  input: {
                    id: assetId,
                    revision: 1,
                    status: Number(data.get("status") ?? 200),
                    headers: responseHeaders,
                    bodyPath: this.responseBodyPath,
                    bodyFileMode: this.responseBodyFileMode,
                    mediaType: optionalText(data.get("mediaType")),
                  },
                })
              : await invoke<ResponseAsset>("create_response_asset", {
                  input: {
                    id: assetId,
                    revision: 1,
                    status: Number(data.get("status") ?? 200),
                    headers: responseHeaders,
                    body: this.currentBodyBytes(),
                    mediaType: optionalText(data.get("mediaType")),
                  },
                });
        if (
          source.kind === "captured" &&
          (String(data.get("responseHeaders") ?? "") !== this.savedHeaderText ||
            Number(data.get("status")) !== asset.status ||
            String(data.get("mediaType") ?? "") !== (asset.mediaType ?? ""))
        ) {
          asset = await invoke<ResponseAsset>("edit_response_asset", {
            input: {
              assetReference: `${asset.id}@${asset.revision}`,
              status: Number(data.get("status")),
              headers: responseHeaders,
              mediaType: optionalText(data.get("mediaType")),
              decodedBody: null,
              bodyPath: null,
              preserveContentEncoding: preserveEncoding,
            },
          });
        }
        assetReference = `${asset.id}@${asset.revision}`;
      }

      const id =
        existing?.id ??
        (this.stagingCollection
          ? this.collectionRuleId
          : `autoresponse-rule-${crypto.randomUUID().replaceAll("-", "")}`);
      const rule: AutomationRule = {
        ...template,
        id,
        displayName,
        enabled: experiment ? (existing?.enabled ?? true) : this.autoResponseEnabled.checked,
        revision: (existing?.revision ?? 0) + 1,
        priority: existing?.priority ?? AUTORESPONSE_PRIORITY_BASE,
        matcher,
        request: {
          ...(template?.request ?? {
            headers: [],
            replaceBody: null,
            discardBody: false,
            abortReason: null,
            allowNonIdempotentBodyReplacement: false,
          }),
          responseAsset: assetReference,
        },
        response: template?.response ?? {
          headers: [],
          replaceBody: null,
          discardBody: false,
          abortReason: null,
        },
      };
      const ordered = this.autoResponseRules(current).filter((candidate) => candidate.id !== id);
      if (existing === undefined) {
        ordered.unshift(rule);
      } else {
        ordered.splice(
          Math.max(
            0,
            this.autoResponseRules(current).findIndex((candidate) => candidate.id === id),
          ),
          0,
          rule,
        );
      }
      if (this.stagingCollection) {
        this.stagedRule = rule;
        return true;
      }
      const status = await this.activateAutoResponseOrder(current, ordered);
      if (!experiment) {
        this.rememberRuleChange(
          this.autoResponseRules(current),
          this.autoResponseRules(status),
          existing ? "Saved rule properties." : "Created a new rule.",
        );
      }
      this.renderAutomation(status);
      this.savingAutoResponse = false;
      this.draftDirty = false;
      this.automationKind = "success";
      {
        await this.cancelAutoResponseEdit();
        this.ruleSelection.replace(id);
        this.renderRuleSelection();
        this.automationText = `Saved “${rule.displayName}”${existing === undefined ? " at the top" : ""}. The first enabled matching rule will win.`;
        this.focusRule(id);
        const saved = status.rules.find((rule) => rule.id === id);
        if (saved) {
          await this.editAutoResponse(saved, true);
        }
      }
      return true;
    } catch (error: unknown) {
      this.automationText = `Saved response could not be saved: ${describeError(error)}`;
      this.automationKind = "error";
      this.autoResponseError = describeError(error);
      return false;
    } finally {
      this.savingAutoResponse = false;
      this.$flushUpdates();
      if (this.requestHeadersError) {
        (this.autoResponseForm.elements.namedItem("requestHeaders") as HTMLTextAreaElement).focus();
      } else if (this.responseHeadersError) {
        (
          this.autoResponseForm.elements.namedItem("responseHeaders") as HTMLTextAreaElement
        ).focus();
      }
    }
  }

  private autoResponseRules(status: AutomationStatus): AutomationRule[] {
    const owned = experimentRuleIds(status);
    return status.rules
      .filter((rule) => rule.request.responseAsset !== null && !owned.has(rule.id))
      .sort((left, right) => left.priority - right.priority || left.id.localeCompare(right.id));
  }

  private async activateAutoResponseOrder(
    current: AutomationStatus,
    ordered: AutomationRule[],
  ): Promise<AutomationStatus> {
    const normalized = ordered.map((rule, index) => {
      const priority = AUTORESPONSE_PRIORITY_BASE + index;
      return rule.priority === priority ? rule : { ...rule, revision: rule.revision + 1, priority };
    });
    const ids = new Set(normalized.map((rule) => rule.id));
    const ordinary = new Set(this.autoResponseRules(current).map((r) => r.id));
    const other = current.rules.filter((rule) => !ordinary.has(rule.id) && !ids.has(rule.id));
    return this.activateRuleSet([...normalized, ...other], current.generation);
  }

  async mutateAutoResponse(
    ruleId: string,
    mutation: "toggle" | "remove" | "up" | "down",
  ): Promise<void> {
    this.ruleSelection.replace(ruleId);
    this.renderRuleSelection();
    const enabled =
      this.automationStatus?.rules.find((rule) => rule.id === ruleId)?.enabled ?? true;
    await this.mutateSelectedRules(
      mutation === "toggle" ? (enabled ? "disable" : "enable") : mutation,
    );
  }

  private focusRule(id: string): void {
    this.$flushUpdates();
    const element = (this.getRootNode() as ShadowRoot).getElementById("rule-" + id);
    element?.scrollIntoView({ block: "nearest" });
    element?.focus({ preventScroll: true });
  }

  async undoRemoveAutoResponse(): Promise<void> {
    const action = this.ruleHistory.at(-1);
    if (!action || this.savingAutoResponse || this.changingAutoResponse) {
      return;
    }
    const activationOnly =
      action.before.length === action.after.length &&
      action.before.every((rule) => {
        const after = action.after.find((item) => item.id === rule.id);
        return after && sameRuleContent(rule, after);
      });
    if (!activationOnly && !(await this.canLeaveEditor())) {
      return;
    }
    this.changingAutoResponse = true;
    try {
      const current = await invoke<AutomationStatus>("automation_status");
      if (
        JSON.stringify(action.all ? current.rules : this.autoResponseRules(current)) !==
        JSON.stringify(action.after)
      ) {
        throw new Error("Rules changed outside this action. Refresh before undoing.");
      }
      const ordered = action.before.map((rule) => {
        const existing = current.rules.find((item) => item.id === rule.id);
        return action.all && JSON.stringify(rule) === JSON.stringify(existing)
          ? existing!
          : { ...rule, revision: Math.max(rule.revision, existing?.revision ?? 0) + 1 };
      });
      const status = await (action.all
        ? this.activateRuleSet(ordered, current.generation)
        : this.activateAutoResponseOrder(current, ordered));
      this.ruleHistory.pop();
      const earlier = this.ruleHistory.at(-1);
      if (earlier) {
        earlier.after = structuredClone(
          earlier.all ? status.rules : this.autoResponseRules(status),
        );
      }
      this.undoRemoveText = earlier?.label ?? "";
      this.renderAutomation(status);
      const restored = ordered
        .filter((rule) => !action.after.some((item) => item.id === rule.id))
        .map((rule) => rule.id);
      const selected = restored.length
        ? restored
        : ordered.filter((rule) => this.ruleSelection.ids.has(rule.id)).map((rule) => rule.id);
      this.ruleSelection.ids = new Set(selected);
      this.renderRuleSelection();
      this.changingAutoResponse = false;
      if (!activationOnly && selected.length === 1) {
        const rule = status.rules.find((rule) => rule.id === selected[0]);
        if (rule) {
          await this.openRuleProperties(rule);
        }
      }
      if (!activationOnly && selected[0]) {
        this.focusRule(selected[0]);
      }
    } catch (error: unknown) {
      this.automationText = "Rule could not be restored: " + describeError(error);
      this.automationKind = "error";
    } finally {
      this.changingAutoResponse = false;
    }
  }

  async editAutoResponse(
    rule: AutomationRule,
    force = false,
    experiment = false,
    variant?: ExperimentSelection,
  ): Promise<void> {
    if (this.savingAutoResponse) {
      return;
    }
    if (!force && rule.id === this.editingAutoResponseId && !this.editorHidden) {
      this.showAutomationSection(this.responseExperiment ? "experiments" : "responses");
      return;
    }
    if (!force && !(await this.canLeaveEditor())) {
      return;
    }
    this.prepareEditor(experiment);
    this.responseRuleTemplate = structuredClone(rule);
    this.editingVariant = variant ?? null;
    const revision = this.editorRevision;
    const url =
      rule.matcher.url?.kind === "exact"
        ? rule.matcher.url.value
        : rule.matcher.url?.kind === "pattern"
          ? rule.matcher.url.value.address
          : "https://" + (rule.matcher.host ?? "example.test") + (rule.matcher.pathPrefix ?? "/");
    this.editingAutoResponseId = rule.id;
    this.savedActivationRule = experiment ? null : rule;
    this.renderExperimentState();
    this.autoResponseSourceState = {
      kind: "existing",
      assetReference: rule.request.responseAsset ?? "",
    };
    const elements = this.autoResponseForm.elements;
    (elements.namedItem("name") as HTMLInputElement).value = rule.displayName ?? "Saved response";
    this.setEditorMethod(rule.matcher.method);
    (elements.namedItem("url") as HTMLInputElement).value = url;
    this.matcherCondition = rule.matcher.url ?? null;
    this.matchExamples = rule.matcher.examples ?? [];
    this.matchTestUrl.value = url;
    this.autoResponseEnabled.checked = rule.enabled ?? true;
    this.matchTestMethod.value = rule.matcher.method ?? "GET";
    const extras = [
      rule.matcher.scheme ? `Scheme: ${rule.matcher.scheme}` : "",
      rule.matcher.host ? `Host: ${rule.matcher.host}` : "",
      rule.matcher.port ? `Port: ${rule.matcher.port}` : "",
      rule.matcher.pathPrefix ? `Path prefix: ${rule.matcher.pathPrefix}` : "",
      rule.matcher.query !== null ? `Exact query: ${rule.matcher.query}` : "",
      ...rule.matcher.requestHeaders
        .filter((header) => header.condition.kind !== "exact")
        .map((header) => `${header.name}: ${header.condition.kind}`),
    ].filter(Boolean);
    this.advancedMatcherText = extras.join("\n");
    this.ruleSelection.replace(rule.id);
    this.renderRuleSelection();
    (elements.namedItem("requestHeaders") as HTMLTextAreaElement).value =
      rule.matcher.requestHeaders
        .filter((header) => header.condition.kind === "exact")
        .map(
          (header) =>
            `${header.name}: ${new TextDecoder().decode(new Uint8Array(Array.isArray(header.condition.value) ? header.condition.value : []))}`,
        )
        .join("\n");
    this.matchTestHeaders.value = (
      elements.namedItem("requestHeaders") as HTMLTextAreaElement
    ).value;
    this.editorTitle = `Edit “${rule.displayName ?? "Saved response"}”`;
    this.sourceText = "Loading saved response…";
    this.responseReadOnly = true;
    this.responseFieldsHidden = true;
    this.responseHeadersHidden = true;
    this.capturedOptionsHidden = true;
    this.bodyHidden = true;
    this.autoResponseBody.value = "";
    this.bodyReadOnly = true;
    this.loadingSavedResponse = true;
    this.updateAutoResponseHeaderFilterState();
    this.openEditor(experiment && !this.switchingCollection);
    const initialIdentity = JSON.parse(this.nonBodySnapshot()) as Record<string, unknown>;
    try {
      const inspection = await invoke<ResponseAssetInspection>("inspect_response_asset", {
        reference: rule.request.responseAsset,
      });
      if (revision !== this.editorRevision || !this.isConnected) {
        return;
      }
      const asset = inspection.asset;
      this.savedResponse = inspection;
      this.existingResponse = true;
      this.assetsIndex.set(`${asset.id}@${asset.revision}`, asset);
      this.renderAutoResponseRules(
        this.automationStatus ?? {
          generation: 0,
          rules: [],
          autoresponsesEnabled: true,
          diagnostics: [],
          candidateCount: 0,
          historyCount: 0,
        },
      );
      this.sourceText =
        asset.provenance.kind === "session"
          ? "Saved response created from Traffic. Original details are available through the source link while retained."
          : `${asset.status} · ${formatBytes(asset.bodyBytes)} · ${asset.mediaType ?? "Unknown content type"} · Saved response`;
      this.autoResponseStatus.value = String(asset.status);
      this.autoResponseMediaType.value = asset.mediaType ?? "";
      this.responseReadOnly = false;
      this.responseHeadersHidden = false;
      this.bodyHidden = false;
      const bytes =
        inspection.bytesBase64 != null
          ? decodeBodyBase64(inspection.bytesBase64)
          : inspection.textEncoding !== null
            ? encodeBody(inspection.display, "text", inspection.textEncoding)
            : null;
      if (bytes !== null) {
        this.setBodyContent(bytes, inspection.textEncoding, inspection.display);
      } else {
        this.autoResponseBody.value = inspection.display;
        this.bodyMode = "hex";
        this.bodyByteSummary = inspection.display ? "Hex · Bounded original-byte preview" : "";
      }
      this.bodyReadOnly = bytes === null;
      this.editBodyDisabled = bytes === null;
      this.responseCopiedFrom = asset.copiedFrom ?? "";
      if (asset.linkedBody) {
        this.responseBodyFileMode = "linked";
        this.setResponseFile({ path: asset.linkedBody.path, bytes: asset.bodyBytes, error: null });
        this.responseBodyFileBytes = null;
        this.renderResponseFileDetails();
        void this.refreshResponseFile();
      }
      this.bodyStatusText = inspection.explanation;
      this.savedHeaderText = this.editableHeaders(asset);
      (elements.namedItem("responseHeaders") as HTMLTextAreaElement).value = this.savedHeaderText;
      this.encodingOptionsHidden = inspection.contentCodings.length === 0;
      this.preserveEncodingDisabled = bytes === null;
      this.autoResponsePreserveEncoding.checked = asset.linkedBody
        ? Boolean(asset.linkedBody.applyOriginalCompression)
        : true;
      this.updateCompressionChoice();
      if (this.automationStatus) {
        this.renderRuleUsage(this.automationStatus);
      }
      this.responseFieldsHidden = false;
      const baseline = JSON.parse(this.nonBodySnapshot()) as Record<string, unknown>;
      for (const key of ["name", "method", "url", "requestHeaders", "retainAdvanced", "examples"]) {
        baseline[key] = initialIdentity[key];
      }
      this.savedNonBodySnapshot = JSON.stringify(baseline);
      this.responseBaseline.capture({
        ...(this.responseValues() as object),
        fields: this.savedNonBodySnapshot,
      });
      this.refreshResponseDirty();
      if (asset.provenance.kind === "session") {
        this.sourceSessionId = asset.provenance.exchange_id;
        this.sourceLabel = "Original captured request";
        this.sourceAvailabilityText = "Checking source in Traffic…";
        await this.refreshSourceAvailability();
      }
    } catch (error: unknown) {
      if (revision === this.editorRevision) {
        this.sourceText = "Saved response metadata could not be loaded: " + describeError(error);
      }
    } finally {
      if (revision === this.editorRevision) {
        this.loadingSavedResponse = false;
      }
    }
  }

  async disableBuiltInAutomation(id: string): Promise<void> {
    try {
      const current = await invoke<AutomationStatus>("automation_status");
      const rules = current.rules.filter((rule) => rule.id !== id);
      if (rules.length === current.rules.length) {
        this.renderAutomation(current);
        return;
      }
      const status = await this.activateRuleSet(rules, current.generation);
      this.renderAutomation(status);
      this.diagnostic = "Traffic action disabled for new requests.";
    } catch (error: unknown) {
      this.automationText = `Traffic action could not be disabled: ${describeError(error)}`;
      this.automationKind = "error";
    }
  }

  private async activateRuleSet(
    rules: AutomationStatus["rules"],
    generation: number,
  ): Promise<AutomationStatus> {
    const current = await invoke<AutomationStatus>("automation_status");
    if (current.generation !== generation) {
      throw new Error("Rules changed while you were editing. Refresh before saving.");
    }
    const candidate = await invoke<AutomationCandidate>("validate_automation", {
      document: {
        schemaVersion: 2,
        generation,
        rules,
        autoresponsesEnabled: current.autoresponsesEnabled ?? true,
      },
    });
    return invoke<AutomationStatus>("activate_automation", { candidateId: candidate.candidateId });
  }

  private renderAutomation(status: AutomationStatus): void {
    if (this.automationStatus && status.generation < this.automationStatus.generation) {
      return;
    }
    // Same-generation refreshes may have begun before a page-only metadata save.
    for (const set of status.responseSets ?? []) {
      const previous = this.automationStatus?.responseSets?.find((old) => old.id === set.id);
      if ((previous?.previewPageRevision ?? 0) > (set.previewPageRevision ?? 0)) {
        set.previewPage = previous!.previewPage ?? null;
        set.previewPageRevision = previous!.previewPageRevision ?? 0;
      }
    }
    const matcherStateChanged =
      this.automationStatus !== null && status.generation !== this.automationStatus.generation;
    const previousRule = this.automationStatus?.rules.find(
      (rule) => rule.id === this.editingAutoResponseId,
    );
    const currentRule = status.rules.find((rule) => rule.id === this.editingAutoResponseId);
    // An unrelated completion must not rewrite the currently opened snapshot.
    if (previousRule?.revision !== currentRule?.revision) {
      this.savedActivationRule = currentRule ?? null;
      if (currentRule) {
        this.autoResponseEnabled.checked = currentRule.enabled !== false;
      }
    }
    this.automationStatus = status;
    this.autoresponseState = status;
    this.updateActivity();
    if (matcherStateChanged && !this.editorHidden) {
      this.scheduleMatcherTest();
    }
    this.headerOverrideCount = status.rules.filter(
      (rule) => rule.enabled !== false && rule.request.userAgent,
    ).length;
    this.$emit("automation-state-change", status);
    this.renderAutoResponseRules(status);
    this.renderExperimentState();
  }
}

AutomationWorkspace.define("automation-workspace");
