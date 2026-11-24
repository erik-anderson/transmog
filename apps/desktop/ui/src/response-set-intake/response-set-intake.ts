import { WebUIElement, observable } from "@microsoft/webui-framework";
import { captureFocus, captureRenderScope } from "../rendering.js";
import { invoke } from "@tauri-apps/api/core";
import { ListSelection, isTextEditing } from "../list-selection.js";
import { describeError } from "../utilities.js";
import {
  loadIntakeData,
  queryIntakeTraffic,
  type IntakeData,
  type IntakeTrafficPage,
} from "./source-data.js";
import type {
  AutomationStatus,
  AutomationRule,
  ResponseAsset,
  ResponseSetIntakeRequest,
  ResponseSetIntakeInput,
  ResponseSetIntakeReview,
  ResponseSetIntakeResult,
} from "../models.js";

type SourceRow = {
  id: string;
  inResults: boolean;
  name: string;
  method: string;
  url: string;
  response: string;
  bodySource: string;
  eligible: boolean;
  reason: string;
  selected: boolean;
  tabIndex: string;
};
export interface IntakeDraftResolution {
  beforeCommit?: () => boolean;
  afterCommit: (status: AutomationStatus) => Promise<void>;
}
interface IntakeHost {
  prepare: (
    id: string,
    current: () => boolean,
    movingRuleIds: string[],
  ) => Promise<IntakeDraftResolution | null>;
  complete: (status: AutomationStatus) => void;
}
function ownedIds(status: AutomationStatus): Set<string> {
  return new Set(
    (status.responseSets ?? []).flatMap((set) =>
      [
        ...set.rules,
        ...set.versions.flatMap((version) => version.rules),
        ...(set.applied?.rules ?? []),
        ...(set.undoApply?.applied?.rules ?? []),
      ].map((rule) => rule.id),
    ),
  );
}
function ruleUrl(rule: AutomationRule): string {
  return rule.matcher.url?.kind === "exact"
    ? rule.matcher.url.value
    : rule.matcher.url?.kind === "pattern"
      ? rule.matcher.url.value.address
      : rule.matcher.url?.kind === "regex"
        ? rule.matcher.url.value.pattern
        : (rule.matcher.host ?? "Any URL");
}

/** One explicit picker and review shared by Traffic, Rules and response-set workspaces. */
export class ResponseSetIntake extends WebUIElement {
  @observable intakeSource = "traffic";
  @observable intakeOpen = false;
  @observable intakeRows: SourceRow[] = [];
  @observable intakeDestinations: Array<{ id: string; name: string }> = [];
  @observable intakeDestination = "";
  @observable intakeName = "";
  @observable intakeNotes = "";
  @observable intakeSearch = "";
  @observable intakeMode = "copy";
  @observable intakeLoading = false;
  @observable intakeBodyLoading = false;
  @observable intakeBodyFailed = false;
  @observable intakeDataFailed = false;
  @observable intakeReviewBusy = false;
  @observable intakeSaving = false;
  @observable intakeReview: ResponseSetIntakeReview | null = null;
  @observable intakeError = "";
  @observable intakeSelectedCount = 0;
  @observable intakeMore = false;
  @observable intakeAcknowledged = false;
  @observable intakeResult = "";
  @observable intakeCompleted = false;
  @observable intakeSubmitLabel = "Add responses";
  @observable intakeReviewSummary = "";
  intakeDialog!: HTMLDialogElement;
  sourceSearch!: HTMLInputElement;
  intakeNameInput!: HTMLInputElement;
  intakeNotesInput!: HTMLTextAreaElement;
  destinationSelect!: HTMLSelectElement;
  submitButton!: HTMLButtonElement;
  closeButton!: HTMLButtonElement;
  private epoch = 0;
  private loadEpoch = 0;
  private offset = 0;
  private selection = new ListSelection();
  private status: AutomationStatus | null = null;
  private assets: ResponseAsset[] = [];
  private host: IntakeHost | null = null;
  private opening: { request: ResponseSetIntakeRequest; data: Promise<IntakeData> } | null = null;
  private reviewedInput: ResponseSetIntakeInput | null = null;
  private resolution: IntakeDraftResolution | null = null;
  private resultId = "";
  private returnFocus: HTMLElement | null = null;
  private searchTimer: number | undefined;

  constructor() {
    super();
    this.addEventListener("lazyerror", (event) => {
      // The picker shows an error and retry for its own template pack.
      if (event.target === this) {
        event.preventDefault();
      }
    });
  }

  private active(): Element | null {
    const root = this.getRootNode();
    return root instanceof ShadowRoot ? root.activeElement : document.activeElement;
  }
  private current(epoch = this.epoch): boolean {
    return this.isConnected && epoch === this.epoch && this.intakeDialog.open;
  }
  async request(request: ResponseSetIntakeRequest, host: IntakeHost): Promise<void> {
    if (this.intakeSaving) {
      return;
    }
    if (this.intakeDialog.open) {
      this.close();
    }
    const epoch = ++this.epoch,
      active = this.active();
    this.returnFocus = active instanceof HTMLElement ? active : null;
    this.host = host;
    this.status = null;
    this.assets = [];
    this.intakeDestinations = [];
    this.intakeSource = request.source;
    this.selection.clear();
    this.selection.ids = new Set(request.ids ?? []);
    this.intakeRows = [];
    this.intakeSearch = "";
    this.intakeDestination = request.destinationId ?? "";
    this.intakeName = "";
    this.intakeNotes = "";
    this.intakeMode = "copy";
    this.intakeError = "";
    this.intakeReview = null;
    this.reviewedInput = null;
    this.resolution = null;
    this.intakeCompleted = false;
    this.offset = 0;
    this.intakeMore = false;
    this.updateSelection();
    this.intakeOpen = true;
    this.intakeDialog.showModal();
    this.closeButton.focus({ preventScroll: true });
    this.beginOpening(request);
    await this.loadIntakeBody(epoch);
  }
  private beginOpening(request: ResponseSetIntakeRequest): void {
    const data = loadIntakeData(request);
    // The read can settle before the template pack loads or after the picker closes.
    void data.catch(() => undefined);
    this.opening = { request, data };
  }
  async retryIntake(): Promise<void> {
    if (!this.opening || !this.current()) {
      return;
    }
    const epoch = this.epoch;
    if (this.intakeBodyFailed) {
      await this.loadIntakeBody(epoch);
    } else if (this.intakeDataFailed) {
      this.beginOpening({ ...this.opening.request, ids: [...this.selection.ids] });
      this.sourceSearch.focus({ preventScroll: true });
      await this.loadOpeningData(epoch);
    }
  }
  private async loadIntakeBody(epoch: number): Promise<void> {
    const focus = captureFocus(this);
    this.intakeError = "";
    this.intakeBodyFailed = false;
    this.intakeBodyLoading = true;
    this.intakeLoading = true;
    try {
      await this.$whenRendered();
    } catch {
      if (this.current(epoch)) {
        this.intakeBodyLoading = false;
        this.intakeBodyFailed = true;
        this.intakeLoading = false;
        this.intakeError = "The response picker could not be loaded. Try again.";
      }
      return;
    }
    if (!this.current(epoch)) {
      return;
    }
    this.intakeBodyLoading = false;
    this.sourceSearch.value = "";
    this.intakeNameInput.value = "";
    this.intakeNotesInput.value = "";
    if (focus()) {
      this.sourceSearch.focus({ preventScroll: true });
    }
    await this.loadOpeningData(epoch);
  }
  private async loadOpeningData(epoch: number): Promise<void> {
    const opening = this.opening;
    if (!opening) {
      return;
    }
    const { data } = opening;
    const load = this.loadEpoch;
    this.intakeLoading = true;
    this.intakeDataFailed = false;
    this.intakeError = "";
    try {
      const { status, assets, traffic } = await data;
      if (!this.current(epoch)) {
        return;
      }
      this.opening = null;
      this.assets = assets;
      const missingDestination = this.applyStatus(status);
      if (traffic && !this.intakeSearch) {
        this.applyTrafficPage(traffic, false);
      } else {
        await this.loadSources();
      }
      if (
        missingDestination &&
        this.current(epoch) &&
        !this.intakeDestination &&
        !this.intakeError
      ) {
        this.intakeError = "The selected response set is no longer available.";
      }
    } catch (error) {
      if (this.current(epoch)) {
        this.intakeDataFailed = this.opening === opening;
        this.intakeError = describeError(error);
      }
    } finally {
      if (this.current(epoch) && load === this.loadEpoch) {
        this.intakeLoading = false;
        this.updateSelection();
      }
    }
  }
  private applyStatus(status: AutomationStatus): boolean {
    this.status = status;
    this.intakeDestinations = (status.responseSets ?? []).map(({ id, name }) => ({ id, name }));
    if (
      this.intakeDestination &&
      this.intakeDestination !== "__new" &&
      !this.intakeDestinations.some((set) => set.id === this.intakeDestination)
    ) {
      this.intakeDestination = "";
      return true;
    }
    return false;
  }
  private ordinaryRows(): SourceRow[] {
    if (!this.status) {
      return [];
    }
    const owned = ownedIds(this.status),
      query = this.intakeSearch.toLocaleLowerCase();
    return this.status.rules
      .filter((rule) => !!rule.request.responseAsset && !owned.has(rule.id))
      .sort((a, b) => a.priority - b.priority || a.id.localeCompare(b.id))
      .map((rule) => {
        const asset = this.assets.find(
            (asset) => asset.id + "@" + asset.revision === rule.request.responseAsset,
          ),
          url = ruleUrl(rule),
          name = rule.displayName ?? rule.id;
        return {
          id: rule.id,
          inResults: (name + " " + url).toLocaleLowerCase().includes(query),
          name,
          method: rule.matcher.method ?? "ANY",
          url,
          response: asset
            ? `${asset.status} · ${asset.bodyBytes.toLocaleString()} bytes`
            : "Saved response unavailable",
          bodySource: asset?.linkedBody
            ? "Linked file: " + asset.linkedBody.path
            : asset?.copiedFrom
              ? "Copied file: " + asset.copiedFrom
              : "Saved response bytes",
          eligible: !!asset,
          reason: asset
            ? "Original inclusion: " + (rule.enabled ? "Included" : "Excluded")
            : "Saved response asset unavailable",
          selected: false,
          tabIndex: "-1",
        };
      });
  }
  private applyTrafficPage(page: IntakeTrafficPage, more: boolean): void {
    const rows = page.rows.map((row): SourceRow => ({
      id: row.id,
      inResults: row.inPage,
      name:
        row.requestNumber === null
          ? "Unavailable captured response"
          : `#${row.requestNumber} · ${row.method ?? "Request"}`,
      method: row.method ?? "",
      url: row.url,
      response:
        row.requestNumber === null
          ? "Unavailable"
          : `${row.status ?? "No response"} · ${row.retainedBytes.toLocaleString()} bytes`,
      bodySource: row.requestNumber === null ? "" : "Copy complete captured bytes",
      eligible: row.unavailableReason === null,
      reason: row.unavailableReason ?? "Ready",
      selected: false,
      tabIndex: "-1",
    }));
    this.intakeRows = more
      ? [...new Map([...this.intakeRows, ...rows].map((row) => [row.id, row])).values()]
      : rows;
    this.offset = page.nextOffset ?? 0;
    this.intakeMore = page.nextOffset !== null;
  }
  async loadSources(more = false): Promise<void> {
    if (more && (this.intakeLoading || !this.intakeMore)) {
      return;
    }
    const epoch = this.epoch,
      load = ++this.loadEpoch;
    this.intakeLoading = true;
    this.intakeError = "";
    this.invalidateReview();
    try {
      if (this.intakeSource === "rules") {
        this.intakeRows = this.ordinaryRows();
        this.intakeMore = false;
        return;
      }
      const selectedIds = more
        ? []
        : [...new Set([...this.intakeRows.map((row) => row.id), ...this.selection.ids])].filter(
            (id) => this.selection.ids.has(id),
          );
      const page = await queryIntakeTraffic(more ? this.offset : 0, this.intakeSearch, selectedIds);
      if (!this.current(epoch) || load !== this.loadEpoch) {
        return;
      }
      this.applyTrafficPage(page, more);
    } catch (error) {
      if (this.current(epoch) && load === this.loadEpoch) {
        if (!more) {
          this.intakeRows = this.intakeRows.map((row) => ({ ...row, inResults: false }));
        }
        this.intakeError = describeError(error);
      }
    } finally {
      if (this.current(epoch) && load === this.loadEpoch) {
        this.intakeLoading = false;
        this.updateSelection();
      }
    }
  }
  sourceSearchChanged(event: Event): void {
    this.intakeSearch = (event.currentTarget as HTMLInputElement).value;
    window.clearTimeout(this.searchTimer);
    if (this.status) {
      // The input changes before the debounced query starts. Its previous
      // results and pagination must lose ownership immediately.
      this.loadEpoch++;
      this.intakeLoading = true;
      this.intakeMore = false;
      this.invalidateReview();
      this.searchTimer = window.setTimeout(() => void this.loadSources(), 180);
    }
  }
  changeDestination(event: Event): void {
    this.intakeDestination = (event.currentTarget as HTMLSelectElement).value;
    this.invalidateReview();
  }
  changeMetadata(field: string, event: Event): void {
    const value = (event.currentTarget as HTMLInputElement).value;
    if (field === "name") {
      this.intakeName = value;
    } else {
      this.intakeNotes = value;
    }
    this.invalidateReview();
  }
  changeMode(event: Event): void {
    this.intakeMode = (event.currentTarget as HTMLInputElement).value;
    this.invalidateReview();
  }
  private invalidateReview(): void {
    this.intakeReview = null;
    this.reviewedInput = null;
    this.resolution = null;
    this.intakeAcknowledged = false;
  }
  private order(): string[] {
    return this.intakeRows.filter((row) => row.eligible).map((row) => row.id);
  }
  private updateSelection(): void {
    const rows = this.intakeRows.filter((row) => row.inResults || this.selection.ids.has(row.id));
    if (rows.length < this.intakeRows.length) {
      const activeRow = this.active()?.closest("tr")?.id;
      if (
        activeRow?.startsWith("intake-source-") &&
        !rows.some((row) => "intake-source-" + row.id === activeRow)
      ) {
        this.sourceSearch.focus({ preventScroll: true });
      }
    }
    if (this.selection.focused && !rows.some((row) => row.id === this.selection.focused)) {
      this.selection.focused = null;
    }
    const focus = this.selection.focused ?? rows.find((row) => row.eligible)?.id;
    this.intakeRows = rows.map((row) => ({
      ...row,
      selected: this.selection.ids.has(row.id),
      tabIndex: row.id === focus ? "0" : "-1",
    }));
    this.intakeSelectedCount = this.selection.ids.size;
  }
  selectRow(id: string, event: MouseEvent, checkbox = false): void {
    const input = checkbox ? (event.currentTarget as HTMLInputElement) : null;
    const row = this.intakeRows.find((row) => row.id === id);
    if (
      this.intakeLoading ||
      this.intakeReviewBusy ||
      this.intakeSaving ||
      (!row?.eligible && !this.selection.ids.has(id))
    ) {
      if (input) {
        event.preventDefault();
      }
      return;
    }
    event.stopPropagation();
    const modifiers = !row?.eligible
      ? { ctrlKey: true }
      : input
        ? { ctrlKey: true, shiftKey: event.shiftKey }
        : event;
    this.selection.choose(id, this.order(), modifiers);
    // A range can leave an already selected row checked.
    if (input && input.checked !== this.selection.ids.has(id)) {
      event.preventDefault();
    }
    this.invalidateReview();
    this.updateSelection();
  }
  async rowKeyboard(id: string, event: KeyboardEvent): Promise<void> {
    const target = event.composedPath()[0];
    if (
      isTextEditing(event) ||
      event.key === "Escape" ||
      (event.key === " " && target instanceof HTMLInputElement && target.type === "checkbox")
    ) {
      return;
    }
    if (this.intakeLoading || this.intakeReviewBusy || this.intakeSaving) {
      return;
    }
    if (this.selection.key(id, this.order(), event)) {
      event.stopPropagation();
      this.invalidateReview();
      this.updateSelection();
      const focus = captureFocus(this),
        current = captureRenderScope(this, () => [
          this.epoch,
          this.selection.focused,
          this.intakeReview,
          this.intakeOpen,
        ]);
      await this.$whenRendered();
      if (!current() || !focus() || !this.selection.focused) {
        return;
      }
      const root = this.getRootNode();
      const focused =
        root instanceof ShadowRoot
          ? root.getElementById("intake-source-" + this.selection.focused)
          : document.getElementById("intake-source-" + this.selection.focused);
      focused?.focus({ preventScroll: true });
    }
  }
  clearSelection(event: Event): void {
    const ownsFocus = this.active() === event.currentTarget;
    this.selection.clear();
    this.invalidateReview();
    this.updateSelection();
    if (ownsFocus) {
      this.sourceSearch.focus({ preventScroll: true });
    }
  }
  acknowledge(event: Event): void {
    this.intakeAcknowledged = (event.currentTarget as HTMLInputElement).checked;
  }
  async back(): Promise<void> {
    if (!this.intakeSaving) {
      const focus = captureFocus(this);
      this.invalidateReview();
      const current = captureRenderScope(this, () => [
        this.epoch,
        this.intakeReview,
        this.intakeOpen,
      ]);
      await this.$whenRendered();
      if (current() && focus()) {
        this.sourceSearch.focus({ preventScroll: true });
      }
    }
  }
  async review(): Promise<void> {
    if (
      this.intakeLoading ||
      this.intakeReviewBusy ||
      this.intakeSaving ||
      !this.selection.ids.size ||
      !this.host
    ) {
      return;
    }
    if (this.selection.ids.size > 1000) {
      this.intakeError = "Select at most 1,000 responses.";
      return;
    }
    const ids = this.intakeRows
      .filter((row) => this.selection.ids.has(row.id))
      .map((row) => row.id);
    if (ids.length !== this.selection.ids.size) {
      this.intakeError =
        "Some selected responses are no longer available. Clear the selection and choose responses again.";
      return;
    }
    const epoch = this.epoch,
      source = this.active();
    this.intakeReviewBusy = true;
    this.intakeError = "";
    let focusSubmit: (() => void) | undefined;
    try {
      if (!this.intakeDestination) {
        throw new Error("Choose a response set or create a new response set.");
      }
      const movingRuleIds =
        this.intakeSource === "rules" && this.intakeMode === "move" ? [...this.selection.ids] : [];
      const resolution = await this.host.prepare(
        this.intakeDestination,
        () => this.current(epoch),
        movingRuleIds,
      );
      if (!this.current(epoch)) {
        return;
      }
      if (!resolution) {
        this.intakeError = "Edits were kept. Finish those edits before adding responses.";
        return;
      }
      const status = await invoke<AutomationStatus>("automation_status");
      if (!this.current(epoch)) {
        return;
      }
      this.applyStatus(status);
      const set = status.responseSets?.find((set) => set.id === this.intakeDestination);
      if (this.intakeDestination !== "__new" && !set) {
        throw new Error("The selected response set is no longer available.");
      }
      const input: ResponseSetIntakeInput = {
        generation: status.generation,
        source: {
          kind: this.intakeSource as "traffic" | "rules",
          ids,
        },
        destination: set
          ? { kind: "existing", id: set.id, incarnation: set.incarnation, revision: set.revision }
          : {
              kind: "new",
              id: "set-" + crypto.randomUUID().replaceAll("-", ""),
              name: this.intakeName.trim(),
              notes: this.intakeNotes,
            },
        mode: this.intakeMode as "copy" | "move",
        acknowledgeOverlaps: false,
      };
      const review = await invoke<ResponseSetIntakeReview>("review_response_set_intake", { input });
      if (!this.current(epoch)) {
        return;
      }
      const ownsFocus = this.active() === source;
      const focus = captureFocus(this);
      this.intakeReview = review;
      this.reviewedInput = input;
      this.resolution = resolution;
      this.intakeAcknowledged = false;
      this.intakeSubmitLabel =
        input.mode === "move"
          ? `Move ${review.rows.length} rule${review.rows.length === 1 ? "" : "s"}`
          : "Add responses";
      this.intakeReviewSummary = `Add to ${set?.name ?? this.intakeName.trim()} · ${input.source.kind === "traffic" ? "Copy captured responses" : input.mode === "move" ? "Move rules" : "Copy rules"}`;
      if (ownsFocus) {
        focusSubmit = () => {
          if (this.current(epoch) && this.intakeReview === review && focus()) {
            this.submitButton.focus({ preventScroll: true });
          }
        };
      }
    } catch (error) {
      if (this.current(epoch)) {
        this.intakeError = describeError(error);
      }
    } finally {
      if (this.current(epoch)) {
        this.intakeReviewBusy = false;
      }
    }
    if (focusSubmit) {
      await this.$whenRendered();
      focusSubmit();
    }
  }
  async submit(): Promise<void> {
    if (
      this.intakeSaving ||
      !this.reviewedInput ||
      !this.intakeReview ||
      (this.intakeReview.warnings.length && !this.intakeAcknowledged) ||
      !this.host
    ) {
      return;
    }
    const input = { ...this.reviewedInput, acknowledgeOverlaps: this.intakeAcknowledged },
      epoch = this.epoch,
      resolution = this.resolution,
      host = this.host;
    this.intakeSaving = true;
    this.intakeError = "";
    let focusClose: (() => void) | undefined;
    try {
      if (resolution?.beforeCommit && !resolution.beforeCommit()) {
        throw new Error(
          "A selected source rule changed after review. Review responses again. Your draft is preserved.",
        );
      }
      const result = await invoke<ResponseSetIntakeResult>("collect_response_set", { input });
      host.complete(result.status);
      await resolution?.afterCommit(result.status);
      this.resultId = result.setId;
      this.intakeResult = `Added ${result.addedCount} response${result.addedCount === 1 ? "" : "s"} to ${result.setName}. ${input.mode === "move" ? "Original rules were removed. Saved additions have not been applied." : "Live traffic is unchanged."}`;
      if (this.current(epoch)) {
        const ownsFocus = this.active() === this.submitButton;
        const focus = captureFocus(this);
        this.intakeCompleted = true;
        if (ownsFocus) {
          focusClose = () => {
            if (this.current(epoch) && this.intakeCompleted && focus()) {
              this.closeButton.focus({ preventScroll: true });
            }
          };
        }
      }
    } catch (error) {
      const message = describeError(error);
      if (this.current(epoch)) {
        this.intakeError = message;
      } else {
        this.$emit("operation-error", {
          title: "Responses could not be added",
          message,
          resolve: () => {},
        });
      }
    } finally {
      this.intakeSaving = false;
    }
    if (focusClose) {
      await this.$whenRendered();
      focusClose();
    }
  }
  close(event?: Event): void {
    event?.preventDefault();
    this.epoch++;
    this.loadEpoch++;
    window.clearTimeout(this.searchTimer);
    this.intakeOpen = false;
    this.opening = null;
    this.intakeBodyLoading = false;
    this.intakeBodyFailed = false;
    this.intakeDataFailed = false;
    this.intakeDialog?.close();
    this.intakeLoading = false;
    this.intakeReviewBusy = false;
    const focus = this.returnFocus;
    if (focus?.isConnected && focus.getClientRects().length) {
      focus.focus({ preventScroll: true });
    }
  }
  openResult(): void {
    if (!this.resultId) {
      return;
    }
    this.close();
    this.$emit("response-set-open-request", { id: this.resultId });
  }
  dismissResult(): void {
    this.intakeResult = "";
    this.resultId = "";
  }
  keyboard(event: KeyboardEvent): void {
    event.stopPropagation();
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
    }
  }
  disconnectedCallback(): void {
    this.close();
    super.disconnectedCallback();
  }
}
ResponseSetIntake.define("response-set-intake");
