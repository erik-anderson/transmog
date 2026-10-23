import { WebUIElement, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { ListSelection, isTextEditing } from "../list-selection.js";
import {
  clientResponseSource,
  autoResponseUnavailableReason,
  describeError,
} from "../utilities.js";
import type {
  AutomationStatus,
  AutomationRule,
  ResponseAsset,
  SessionDetail,
  SessionPage,
  ResponseSetIntakeRequest,
  ResponseSetIntakeInput,
  ResponseSetIntakeReview,
  ResponseSetIntakeResult,
} from "../models.js";

type SourceRow = {
  id: string;
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
  private reviewedInput: ResponseSetIntakeInput | null = null;
  private resolution: IntakeDraftResolution | null = null;
  private resultId = "";
  private returnFocus: HTMLElement | null = null;
  private searchTimer: number | undefined;
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
    this.intakeLoading = true;
    this.offset = 0;
    this.intakeMore = false;
    this.updateSelection();
    this.intakeOpen = true;
    this.$flushUpdates();
    this.sourceSearch.value = "";
    this.intakeNameInput.value = "";
    this.intakeNotesInput.value = "";
    this.intakeDialog.showModal();
    this.sourceSearch.focus({ preventScroll: true });
    try {
      const [status, assets] = await Promise.all([
        invoke<AutomationStatus>("automation_status"),
        invoke<ResponseAsset[]>("response_assets"),
      ]);
      if (!this.current(epoch)) {
        return;
      }
      this.status = status;
      this.assets = assets;
      this.intakeDestinations = (status.responseSets ?? []).map((set) => ({
        id: set.id,
        name: set.name,
      }));
      if (
        request.destinationId &&
        !this.intakeDestinations.some((set) => set.id === request.destinationId)
      ) {
        throw new Error("The selected response set is no longer available.");
      }
      await this.loadSources(false, request.ids ?? []);
    } catch (error) {
      if (this.current(epoch)) {
        this.intakeError = describeError(error);
      }
    } finally {
      if (this.current(epoch)) {
        this.intakeLoading = false;
      }
    }
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
      })
      .filter(
        (row) =>
          this.selection.ids.has(row.id) ||
          (row.name + " " + row.url).toLocaleLowerCase().includes(query),
      );
  }
  async loadSources(more = false, initialIds: string[] = []): Promise<void> {
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
      const offset = more ? this.offset : 0;
      const page = await invoke<SessionPage>("query_sessions", {
        query: {
          limit: 100,
          offset,
          search: this.intakeSearch,
          sort: { column: "request-number", direction: "descending" },
        },
      });
      if (!this.current(epoch) || load !== this.loadEpoch) {
        return;
      }
      const preserved = this.intakeRows.filter((row) => this.selection.ids.has(row.id));
      const ids = [...new Set([...initialIds, ...page.sessions.map((row) => row.id)])];
      const rows: SourceRow[] = [];
      for (let start = 0; start < ids.length; start += 8) {
        const loaded = await Promise.all(
          ids.slice(start, start + 8).map(async (id) => {
            try {
              const detail = await invoke<SessionDetail>("session_detail", { id }),
                request = detail.requests.find((head) => head.boundary === "client-request"),
                source = clientResponseSource(detail),
                metadata = detail.storedBodies.find((body) => body.boundary === "client-response"),
                status = detail.responses.find(
                  (head) => head.boundary === "client-response",
                )?.status;
              return {
                id,
                name: `#${detail.requestNumber} · ${request?.method ?? "Request"}`,
                method: request?.method ?? "",
                url: request?.target ?? "",
                response: `${status ?? "No response"} · ${(metadata?.retainedBytes ?? 0).toLocaleString()} bytes`,
                bodySource: "Copy complete captured bytes",
                eligible: !!source,
                reason: source ? "Ready" : autoResponseUnavailableReason(detail),
                selected: false,
                tabIndex: "-1",
              };
            } catch (error) {
              return {
                id,
                name: "Unavailable captured response",
                method: "",
                url: "",
                response: "Unavailable",
                bodySource: "",
                eligible: false,
                reason: describeError(error),
                selected: false,
                tabIndex: "-1",
              };
            }
          }),
        );
        if (!this.current(epoch) || load !== this.loadEpoch) {
          return;
        }
        rows.push(...loaded);
      }
      const merged = more ? [...this.intakeRows, ...rows] : [...preserved, ...rows];
      this.intakeRows = [...new Map(merged.map((row) => [row.id, row])).values()];
      this.offset = offset + page.sessions.length;
      this.intakeMore = this.offset < page.totalMatched;
    } catch (error) {
      if (this.current(epoch) && load === this.loadEpoch) {
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
    this.searchTimer = window.setTimeout(() => void this.loadSources(), 180);
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
    const focus = this.selection.focused ?? this.intakeRows.find((row) => row.eligible)?.id;
    this.intakeRows = this.intakeRows.map((row) => ({
      ...row,
      selected: this.selection.ids.has(row.id),
      tabIndex: row.id === focus ? "0" : "-1",
    }));
    this.intakeSelectedCount = this.selection.ids.size;
  }
  selectRow(id: string, event: MouseEvent, checkbox = false): void {
    if (this.intakeLoading || this.intakeReviewBusy || this.intakeSaving) {
      return;
    }
    const row = this.intakeRows.find((row) => row.id === id);
    if (!row?.eligible && !this.selection.ids.has(id)) {
      return;
    }
    event.stopPropagation();
    this.selection.choose(
      id,
      this.order(),
      checkbox ? { ctrlKey: true, shiftKey: event.shiftKey } : event,
    );
    this.invalidateReview();
    this.updateSelection();
  }
  rowKeyboard(id: string, event: KeyboardEvent): void {
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
      this.$flushUpdates();
      const root = this.getRootNode();
      const focused =
        root instanceof ShadowRoot
          ? root.getElementById("intake-source-" + this.selection.focused)
          : document.getElementById("intake-source-" + this.selection.focused);
      focused?.focus({ preventScroll: true });
    }
  }
  clearSelection(): void {
    this.selection.clear();
    this.invalidateReview();
    this.updateSelection();
  }
  acknowledge(event: Event): void {
    this.intakeAcknowledged = (event.currentTarget as HTMLInputElement).checked;
  }
  back(): void {
    if (!this.intakeSaving) {
      this.invalidateReview();
      this.$flushUpdates();
      this.sourceSearch.focus({ preventScroll: true });
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
    const epoch = this.epoch,
      source = this.active();
    this.intakeReviewBusy = true;
    this.intakeError = "";
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
      this.status = status;
      const set = status.responseSets?.find((set) => set.id === this.intakeDestination);
      if (this.intakeDestination !== "__new" && !set) {
        throw new Error("The selected response set is no longer available.");
      }
      const input: ResponseSetIntakeInput = {
        generation: status.generation,
        source: {
          kind: this.intakeSource as "traffic" | "rules",
          ids: this.intakeRows.filter((row) => this.selection.ids.has(row.id)).map((row) => row.id),
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
      this.intakeReview = review;
      this.reviewedInput = input;
      this.resolution = resolution;
      this.intakeAcknowledged = false;
      this.intakeSubmitLabel =
        input.mode === "move"
          ? `Move ${review.rows.length} rule${review.rows.length === 1 ? "" : "s"}`
          : "Add responses";
      this.intakeReviewSummary = `Add to ${set?.name ?? this.intakeName.trim()} · ${input.source.kind === "traffic" ? "Copy captured responses" : input.mode === "move" ? "Move rules" : "Copy rules"}`;
      this.$flushUpdates();
      if (ownsFocus) {
        this.submitButton.focus({ preventScroll: true });
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
        this.intakeCompleted = true;
        this.$flushUpdates();
        if (ownsFocus) {
          this.closeButton.focus({ preventScroll: true });
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
  }
  close(event?: Event): void {
    event?.preventDefault();
    this.epoch++;
    this.loadEpoch++;
    window.clearTimeout(this.searchTimer);
    this.intakeOpen = false;
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
