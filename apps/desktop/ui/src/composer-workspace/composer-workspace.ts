import initialState from "../initial-state.json";
import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import type {
  ComposerSource,
  ComposerResult,
  ComposerSnapshot,
  ComposerOrigin,
  ComposerHistoryRequest,
  SessionDetail,
} from "../models.js";
import { describeError, responseTextEncoding, shortUrl } from "../utilities.js";
import type { CertificateViewer } from "../certificate-viewer/certificate-viewer.js";
import type { HeaderEditor, HeaderChangeDetail } from "../header-editor/header-editor.js";
import {
  type ResponseBodyEditor,
  responseBodyLanguage,
  type BodyCommit,
} from "../response-body-editor/response-body-editor.js";
import { bytesToHex, encodeBody, equalBodyBytes, renderBodyBytes } from "../response-body-codec.js";
import type { QueryEditor, QueryChangeDetail } from "../query-editor/query-editor.js";
import "../query-editor/query-editor.js";
import type { ComparisonOpen } from "../comparison-models.js";
import { MeasuredMenu, menuAnchor, navigateMenu } from "../context-menu.js";

interface ComposerHistoryRow {
  id: number;
  label: string;
  status: string;
  sourceId: string;
  sourceLabel: string;
  entryId: string;
  retained: boolean;
  stats: string;
  pressed: string;
}

export class ComposerWorkspace extends WorkspaceElement {
  certificateViewer!: CertificateViewer;
  @attr view = "traffic";
  @attr theme = "system";
  @observable composerText = initialState.composerText;
  @observable composerError = "";
  @observable composerBusy = false;
  @observable composerCancelBusy = false;
  @observable composerElapsed = "";
  @observable composerUrl = "";
  @observable composerDirty = false;
  @observable composerPane = "request";
  @observable composerPanePressed = { request: "true", response: "false" };
  @observable composerResult: ComposerResult | null = null;
  @observable resultInspectionDetail: SessionDetail | null = null;
  @observable resultRequestLabel = "";
  @observable resultTone = "";
  @observable resultMatchesDraft = false;
  @observable resultFromHistory = false;
  @observable resultMessage = "response";
  @observable resultMessagePressed = { request: "false", response: "true" };
  @observable resultRequestTitle = "Prepared request";
  @observable resultStats = "";
  @observable resultSourceId = "";
  @observable resultHistoryId = 0;
  @observable needsNonIdempotent = false;
  @observable needsCredentials = false;
  @observable bodyHexMode = false;
  @observable composerBodyError = "";
  @observable composerBodyByteStatus = "0 bytes";
  @observable composerBodyMaxLength = 8 * 1024 * 1024;
  @observable composerBodyMissing = false;
  @observable composerBodyMode = "inline";
  @observable composerBodyFile = "";
  @observable composerBodySizeLabel = "";
  @observable capturedBodyAvailable = false;
  @observable composerFramingNotice = false;
  @observable composerHistory: ComposerHistoryRow[] = [];
  @observable composerHistoryBusy = false;
  @observable composerHistoryError = "";
  @observable composerActionKind = "result";
  @observable composerAction: ComposerHistoryRow | null = null;
  @observable composerMenuX = "8px";
  @observable composerMenuY = "8px";
  composerActionsMenu!: HTMLElement;
  composerSourcePopover!: HTMLElement;
  composerSourceButton!: HTMLButtonElement;
  private menuReturnFocus: HTMLElement | null = null;
  private readonly menuPlacement = new MeasuredMenu(
    () => this.composerActionsMenu,
    (x, y) => {
      this.composerMenuX = x;
      this.composerMenuY = y;
    },
  );
  private historyVisible = false;
  private historyRevision = 0;
  private capturedBodyId = "";
  private inlineBodyAvailable = true;

  @observable composerSourceId = "";
  @observable composerSourceLabel = "";
  @observable composerSourceAvailable = false;
  composerForm!: HTMLFormElement;
  headerEditor!: HeaderEditor;
  queryEditor!: QueryEditor;
  bodyEditor!: ResponseBodyEditor;
  replaceComposerDialog!: HTMLDialogElement;
  private draftGeneration = 0;
  private draftIdentity = 0;
  private bodyRevision = 0;
  private editVersion = 0;
  private paneRevision = 0;
  private sendRevision = 0;
  private sourceRevision = 0;
  private resultRevision = 0;
  private messageRevision = 0;
  private historyActionRevision = 0;
  private sendOperation = "";
  private sendGeneration = 0;
  private cancelRequested = false;
  private elapsedTimer = 0;
  private bodyTextEncoding: string | null = "utf-8";
  private bodyViewValue = "";
  private bodyViewBytes: number[] | null = null;
  private bodyEditorDraft: { generation: number; revision: number; editVersion: number } | null =
    null;
  private leaveResolve: ((value: boolean) => void) | null = null;
  private draftFeedback = false;
  protected hydratedCallback(): void {
    this.updateRisk();
    this.updateBodyStatus();
  }
  viewChanged(): void {
    if (this.view !== "composer") {
      this.historyActionRevision++;
      this.closeComposerActions(false);
      this.composerSourcePopover?.hidePopover();
    }
    if (this.view === "composer" && this.composerSourceId) {
      void this.checkSource();
    }
  }
  private input(name: string): HTMLInputElement {
    return this.composerForm.elements.namedItem(name) as HTMLInputElement;
  }
  markComposerDirty(event: Event): void {
    // Header editor owns its native input events and emits one complete draft change.
    if (
      event.composedPath().includes(this.headerEditor) ||
      event.composedPath().includes(this.queryEditor)
    ) {
      return;
    }
    const name = (event.target as HTMLInputElement).name;
    if (name === "bodyView") {
      return;
    }
    this.markDirty();
    if (name === "url") {
      this.composerUrl = this.input("url").value;
      this.validateUrlLength();
    }
    if (name === "bodyMode") {
      this.composerBodyMode = this.input("bodyMode").value;
      this.bodyRevision++;
      this.updateBodyAvailability();
    }
    if (name === "body") {
      this.bodyRevision++;
      this.inlineBodyAvailable = true;
      this.updateBodyAvailability();
      this.updateBodyStatus();
    }
    if (name === "method") {
      this.input("nonIdempotent").checked = false;
    }
    this.updateRisk();
  }
  queryChanged(event: CustomEvent<QueryChangeDetail>): void {
    event.stopPropagation();
    this.composerUrl = event.detail.value;
    this.input("url").value = event.detail.value;
    this.validateUrlLength();
    this.markDirty();
  }
  private validateUrlLength(): void {
    const input = this.input("url");
    input.setCustomValidity(
      input.value.length > input.maxLength
        ? `URL exceeds the ${input.maxLength.toLocaleString()} character limit. Shorten it before sending.`
        : "",
    );
  }
  composerKeydown(event: KeyboardEvent): void {
    if (
      event.key === "Enter" &&
      event.ctrlKey &&
      !event.altKey &&
      !event.metaKey &&
      !event.shiftKey &&
      !event.isComposing
    ) {
      event.preventDefault();
      event.stopPropagation();
      if (!event.repeat && !this.composerBusy && !this.composerBodyMissing) {
        this.composerForm.requestSubmit();
      }
    }
  }
  private markDirty(): void {
    if (this.draftFeedback) {
      this.composerText = "";
      this.draftFeedback = false;
    }
    this.composerDirty = true;
    this.editVersion++;
    this.resultMatchesDraft = false;
  }
  headersChanged(event: CustomEvent<HeaderChangeDetail>): void {
    event.stopPropagation();
    this.markDirty();
    this.input("credentials").checked = false;
    if (event.detail.headers) {
      this.updateBodyEncoding(event.detail.headers);
    }
    this.updateRisk();
  }
  private updateRisk(): void {
    this.needsNonIdempotent = !["GET", "HEAD", "PUT", "DELETE", "OPTIONS", "TRACE"].includes(
      this.input("method").value.trim(),
    );
    this.needsCredentials = /^\s*(authorization|proxy-authorization|cookie)\s*:/im.test(
      this.headerEditor.value,
    );
    try {
      this.composerFramingNotice = this.headerEditor
        .getHeaders()
        .some((header) => /^(content-length|transfer-encoding)$/i.test(header.name));
    } catch {
      this.composerFramingNotice = false;
    }
  }
  showComposerPane(pane: string): void {
    this.paneRevision++;
    this.composerPane = pane;
  }
  composerPaneChanged(): void {
    this.composerPanePressed = {
      request: String(this.composerPane === "request"),
      response: String(this.composerPane === "response"),
    };
  }
  resultMessageChanged(): void {
    this.resultMessagePressed = {
      request: String(this.resultMessage === "request"),
      response: String(this.resultMessage === "response"),
    };
  }
  showResultMessage(message: string): void {
    this.messageRevision++;
    this.inspectResultMessage(message);
  }
  private inspectResultMessage(message: string): void {
    this.resultMessage = message;
    const result = this.composerResult;
    this.resultInspectionDetail = result
      ? message === "request"
        ? {
            ...result.detail,
            requests: result.detail.requests.filter(
              (head) => head.boundary === result.sentRequest.requestBoundary,
            ),
          }
        : result.detail
      : null;
  }
  private elapsedLabel(milliseconds: number): string {
    return milliseconds < 1000
      ? `${Math.round(milliseconds).toLocaleString()} ms`
      : `${(milliseconds / 1000).toLocaleString(undefined, { maximumFractionDigits: 2 })} s`;
  }
  private resultStatistics(elapsed: number | null, bytes: number | null): string {
    return [
      elapsed == null ? "" : this.elapsedLabel(elapsed),
      bytes == null ? "" : bytes.toLocaleString() + " B encoded",
    ]
      .filter(Boolean)
      .join(" · ");
  }
  private displayResult(
    result: ComposerResult,
    label: string,
    matchesDraft: boolean,
    fromHistory = false,
    resetMessage = true,
  ): void {
    this.composerResult = result;
    this.resultRequestLabel = label;
    this.resultMatchesDraft = matchesDraft;
    this.resultFromHistory = fromHistory;
    this.resultTone =
      result.status >= 400 ? "failed" : result.status >= 300 ? "redirect" : "success";
    this.resultRequestTitle =
      result.sentRequest.evidence === "observed"
        ? result.sentRequest.requestBoundary === "upstream-request"
          ? "Sent request"
          : "Observed request"
        : "Prepared request";
    this.resultStats = this.resultStatistics(result.elapsedMs, result.responseBodyBytes);
    this.resultSourceId = result.source?.entryId ?? "";
    this.resultHistoryId = result.id;
    this.composerHistory = this.composerHistory.map((row) => ({
      ...row,
      pressed: String(row.id === result.id),
    }));
    this.inspectResultMessage(resetMessage ? "response" : this.resultMessage);
  }
  async openComposerActions(kind: string, event: MouseEvent, id = 0): Promise<void> {
    const eventTarget = event?.currentTarget;
    const result = this.composerResult;
    this.composerActionKind = kind;
    this.composerAction =
      kind === "history"
        ? (this.composerHistory.find((row) => row.id === id) ?? null)
        : result
          ? {
              id: result.id,
              label: this.resultRequestLabel,
              status: String(result.status),
              sourceId: this.resultSourceId,
              sourceLabel: result.source
                ? (result.source.traceName ?? result.source.traceId ?? "Original Traffic request") +
                  " · " +
                  (result.source.originalId ?? result.source.entryId)
                : "Composed request",
              entryId: result.detail.id,
              retained: true,
              stats: this.resultStats,
              pressed: "true",
            }
          : null;
    if (!this.composerAction) {
      return;
    }
    this.menuReturnFocus = eventTarget as HTMLElement;
    await this.$whenRendered();
    this.composerActionsMenu.showPopover({ source: this.menuReturnFocus });
    this.menuPlacement.open(menuAnchor(event, this.menuReturnFocus));
    this.composerActionsMenu
      .querySelector<HTMLButtonElement>("button:not(:disabled):not([hidden])")
      ?.focus();
  }
  closeComposerActions(focus = true): void {
    this.menuPlacement.close();
    this.composerActionsMenu?.hidePopover();
    if (focus && this.menuReturnFocus?.isConnected) {
      this.menuReturnFocus.focus({ preventScroll: true });
    }
  }
  composerActionsToggled(event: ToggleEvent): void {
    if (event.newState === "closed") {
      this.menuPlacement.close();
    }
  }
  composerActionsKeydown(event: KeyboardEvent): void {
    if (event.key === "Tab") {
      this.closeComposerActions();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      this.closeComposerActions();
      return;
    }
    const root = this.getRootNode() as Document | ShadowRoot;
    navigateMenu(
      event,
      [
        ...this.composerActionsMenu.querySelectorAll<HTMLButtonElement>(
          "button:not(:disabled):not([hidden])",
        ),
      ],
      root.activeElement,
    );
  }
  runComposerAction(action: string): void {
    const row = this.composerAction,
      returnFocus = this.menuReturnFocus;
    this.closeComposerActions();
    if (!row) {
      return;
    }
    if (action === "edit") {
      void this.editHistoryRequest(row.id);
    } else if (action === "compare") {
      this.compareWithSource(row.sourceId, row.entryId);
    } else if (action === "source") {
      void this.showHistorySource(row.sourceId);
    } else if (action === "certificates" && returnFocus) {
      void this.certificateViewer.show(row.entryId, returnFocus);
    }
  }
  historyKeydown(id: number, event: KeyboardEvent): void {
    const rows = this.composerHistory.filter((row) => row.retained),
      index = rows.findIndex((row) => row.id === id);
    if (!["ArrowUp", "ArrowDown", "Home", "End"].includes(event.key) || index < 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const next =
        event.key === "Home"
          ? 0
          : event.key === "End"
            ? rows.length - 1
            : Math.max(0, Math.min(rows.length - 1, index + (event.key === "ArrowDown" ? 1 : -1))),
      row = rows[next];
    if (row) {
      (this.getRootNode() as Document | ShadowRoot)
        .getElementById("composer-history-" + row.id)
        ?.focus();
      void this.viewHistoryResponse(row.id);
    }
  }
  compareResultWithSource(): void {
    if (this.composerResult && this.resultSourceId) {
      this.compareWithSource(this.resultSourceId, this.composerResult.detail.id);
    }
  }
  compareWithSource(sourceId: string, entryId: string): void {
    if (sourceId && entryId) {
      this.$emit("comparison-request", {
        left: { kind: "exchange", id: sourceId },
        right: { kind: "exchange", id: entryId },
      } satisfies ComparisonOpen);
    }
  }
  async cancelComposer(): Promise<void> {
    if (!this.composerBusy || this.composerCancelBusy || !this.sendOperation) {
      return;
    }
    const operationId = this.sendOperation;
    this.composerCancelBusy = true;
    this.cancelRequested = true;
    if (this.sendGeneration === this.draftGeneration) {
      this.composerText = "Canceling request…";
    }
    try {
      await invoke("cancel_composer", { operationId });
      if (
        this.sendOperation === operationId &&
        this.composerBusy &&
        this.sendGeneration === this.draftGeneration
      ) {
        this.composerText = "Canceling request…";
      }
    } catch (error: unknown) {
      if (this.sendOperation === operationId && this.isConnected) {
        if (this.sendGeneration === this.draftGeneration) {
          this.composerError = "Cancellation could not be requested: " + describeError(error);
          this.composerText = "Sending request…";
        }
        this.composerCancelBusy = false;
        this.cancelRequested = false;
      }
    }
  }
  useEmptyBody(): void {
    this.composerBodyMode = "inline";
    this.input("bodyMode").value = "inline";
    this.inlineBodyAvailable = true;
    this.composerBodyMissing = false;
    this.setBodyBytes([], this.bodyHexMode);
    this.markDirty();
    this.bodyRevision++;
  }
  private currentBodyBytes(): number[] {
    const value = this.input("body").value;
    return this.bodyViewBytes && value === this.bodyViewValue
      ? this.bodyViewBytes
      : encodeBody(value, this.bodyHexMode ? "hex" : "text", this.bodyTextEncoding);
  }
  private setBodyBytes(bytes: number[], hex = false): void {
    let value: string;
    try {
      value = renderBodyBytes(bytes, hex ? "hex" : "text", this.bodyTextEncoding);
    } catch {
      hex = true;
      value = bytesToHex(bytes);
    }
    this.bodyHexMode = hex;
    this.composerBodyMaxLength = (hex ? 12 : 8) * 1024 * 1024;
    this.input("bodyView").value = hex ? "hex" : "text";
    this.input("body").value = value;
    // Textareas normalize newlines. Keep the original bytes until the user edits.
    this.bodyViewValue = this.input("body").value;
    this.bodyViewBytes = bytes;
    this.updateBodyStatus();
  }
  private encodingForHeaders(headers: Array<{ name: string; value: string }>): string | null {
    const encoded = headers
      .filter((header) => header.name.toLowerCase() === "content-encoding")
      .flatMap((header) => header.value.split(","))
      .some((coding) => {
        const token = coding.trim().toLowerCase();
        return token.length > 0 && token !== "identity";
      });
    return encoded
      ? null
      : responseTextEncoding(
          headers.find((header) => header.name.toLowerCase() === "content-type")?.value ?? null,
        );
  }
  private updateBodyEncoding(headers: Array<{ name: string; value: string }>): void {
    const encoding = this.encodingForHeaders(headers);
    if (encoding === this.bodyTextEncoding) {
      return;
    }
    let bytes: number[] | null = null;
    try {
      bytes = this.currentBodyBytes();
    } catch {
      // An incomplete hex draft must remain editable across header changes.
    }
    this.bodyTextEncoding = encoding;
    if (bytes) {
      this.setBodyBytes(bytes, this.bodyHexMode);
    } else {
      this.updateBodyStatus();
    }
  }
  changeBodyView(event: Event): void {
    event.stopPropagation();
    const hex = (event.currentTarget as HTMLSelectElement).value === "hex";
    if (hex === this.bodyHexMode) {
      return;
    }
    try {
      const bytes = this.currentBodyBytes();
      // A failed conversion keeps both the original representation and input.
      renderBodyBytes(bytes, hex ? "hex" : "text", this.bodyTextEncoding);
      this.setBodyBytes(bytes, hex);
      this.bodyRevision++;
    } catch (error: unknown) {
      this.input("bodyView").value = this.bodyHexMode ? "hex" : "text";
      this.composerBodyError = describeError(error);
    }
  }
  private updateBodyStatus(): void {
    try {
      const count = this.currentBodyBytes().length;
      this.composerBodyByteStatus = `${count.toLocaleString()} ${count === 1 ? "byte" : "bytes"}`;
      this.composerBodyError = "";
    } catch (error: unknown) {
      this.composerBodyByteStatus = "";
      this.composerBodyError = describeError(error);
    }
  }
  async expandBody(event: Event): Promise<void> {
    const eventTarget = event?.currentTarget;
    this.bodyEditorDraft = {
      generation: this.draftGeneration,
      revision: this.bodyRevision,
      editVersion: this.editVersion,
    };
    let bytes: number[] | undefined;
    try {
      bytes = this.currentBodyBytes();
    } catch {
      // The expanded editor also supports repairing invalid hex drafts.
    }
    const value = this.input("body").value;
    let mediaType = "";
    try {
      mediaType =
        this.headerEditor
          .getHeaders()
          .find((header) => header.name.toLowerCase() === "content-type")?.value ?? "";
    } catch {
      // Header validation remains beside the header editor.
    }
    await this.bodyEditor.open(
      value,
      responseBodyLanguage(mediaType, value),
      false,
      eventTarget as HTMLElement,
      {
        mode: this.bodyHexMode ? "hex" : "text",
        textEncoding: this.bodyTextEncoding,
        ...(bytes ? { bytes } : {}),
        title: "Edit request body",
        subject: "Request",
        contentTypeLocation: "request headers",
        saveUnavailableReason: "",
      },
    );
  }
  applyBodyEditor(event: CustomEvent<BodyCommit>): void {
    event.stopPropagation();
    const draft = this.bodyEditorDraft;
    if (
      !draft ||
      draft.generation !== this.draftGeneration ||
      draft.revision !== this.bodyRevision ||
      draft.editVersion !== this.editVersion
    ) {
      this.bodyEditor.bodyError =
        "The request draft changed while this editor was open. Close it and reopen the current body.";
      return;
    }
    const bytes =
      event.detail.bytes ??
      encodeBody(event.detail.value, event.detail.mode, this.bodyTextEncoding);
    this.setBodyBytes(bytes, event.detail.mode === "hex");
    this.inlineBodyAvailable = true;
    this.updateBodyAvailability();
    this.markDirty();
    this.bodyRevision++;
    this.bodyEditor.close();
  }
  bodyEditorClosed(): void {
    this.bodyEditorDraft = null;
  }
  private updateBodyAvailability(): void {
    this.composerBodyMissing =
      this.composerBodyMode === "captured"
        ? !this.capturedBodyAvailable
        : this.composerBodyMode === "file"
          ? !this.composerBodyFile
          : !this.inlineBodyAvailable;
  }
  async chooseBodyFile(): Promise<void> {
    const generation = this.draftGeneration,
      revision = this.bodyRevision;
    try {
      const path = await invoke<string | null>("pick_capture_path", {
        kind: "composer-body",
        format: null,
      });
      if (
        generation !== this.draftGeneration ||
        revision !== this.bodyRevision ||
        !this.isConnected
      ) {
        return;
      }
      if (!path) {
        this.composerText = "Body file selection canceled. Your draft is unchanged.";
        return;
      }
      this.composerBodyFile = path;
      this.composerBodyMode = "file";
      this.input("bodyMode").value = "file";
      this.markDirty();
      this.bodyRevision++;
      this.updateBodyAvailability();
      this.composerText =
        "Replacement body file selected. It will stream when you send the request.";
      this.draftFeedback = true;
    } catch (error: unknown) {
      if (
        generation === this.draftGeneration &&
        revision === this.bodyRevision &&
        this.isConnected
      ) {
        this.composerError = "Body file could not be selected: " + describeError(error);
      }
    }
  }
  historyToggled(event: Event): void {
    this.historyVisible = (event.target as HTMLDetailsElement).open;
    if (this.historyVisible) {
      void this.refreshHistory();
    }
  }
  async refreshHistory(): Promise<void> {
    const revision = ++this.historyRevision;
    this.composerHistoryBusy = true;
    this.composerHistoryError = "";
    try {
      const history = await invoke<ComposerSnapshot[]>("composer_history");
      if (revision !== this.historyRevision || !this.isConnected) {
        return;
      }
      this.composerHistory = history.map((row) => ({
        id: row.id,
        label: row.method + " " + row.target,
        status: row.status == null ? "Failed" : String(row.status),
        sourceId: row.source?.entryId ?? "",
        sourceLabel: row.source
          ? (row.source.traceName ?? row.source.traceId ?? "Live traffic") +
            " · " +
            (row.source.originalId ?? row.source.entryId)
          : "Composed request",
        entryId: row.entryId ?? "",
        retained: row.retained,
        stats: this.resultStatistics(row.elapsedMs, row.responseBodyBytes),
        pressed: String(row.id === this.resultHistoryId),
      }));
    } catch (error: unknown) {
      if (revision === this.historyRevision) {
        this.composerHistoryError = "History could not be loaded: " + describeError(error);
      }
    } finally {
      if (revision === this.historyRevision) {
        this.composerHistoryBusy = false;
      }
    }
  }
  async viewHistoryResponse(id: number): Promise<void> {
    const revision = ++this.resultRevision,
      action = ++this.historyActionRevision,
      paneRevision = this.paneRevision,
      messageRevision = this.messageRevision,
      version = this.editVersion,
      root = this.getRootNode(),
      focusAtStart = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
    this.composerHistoryError = "";
    try {
      const result = await invoke<ComposerResult>("composer_history_result", { id });
      if (
        revision !== this.resultRevision ||
        action !== this.historyActionRevision ||
        !this.isConnected
      ) {
        return;
      }
      const request = result.detail.requests.find(
        (head) => head.boundary === result.sentRequest.requestBoundary,
      );
      this.displayResult(
        result,
        request ? (request.method ?? "") + " " + (request.target ?? "") : "Replay " + id,
        false,
        true,
        messageRevision === this.messageRevision,
      );
      if (
        paneRevision === this.paneRevision &&
        version === this.editVersion &&
        (root instanceof ShadowRoot ? root.activeElement : document.activeElement) ===
          focusAtStart &&
        this.view === "composer"
      ) {
        this.composerPane = "response";
      }
    } catch (error: unknown) {
      if (revision === this.resultRevision && action === this.historyActionRevision) {
        this.composerHistoryError = "Response could not be opened: " + describeError(error);
      }
    }
  }
  async editHistoryRequest(id: number): Promise<void> {
    const action = ++this.historyActionRevision,
      generation = this.draftGeneration,
      version = this.editVersion,
      sourceRevision = this.sourceRevision,
      paneRevision = this.paneRevision,
      root = this.getRootNode(),
      focusAtStart = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
    this.composerHistoryError = "";
    let saved: ComposerHistoryRequest;
    try {
      saved = await invoke<ComposerHistoryRequest>("composer_history_request", { id });
    } catch (error: unknown) {
      if (
        action === this.historyActionRevision &&
        generation === this.draftGeneration &&
        sourceRevision === this.sourceRevision &&
        version === this.editVersion &&
        this.isConnected
      ) {
        this.composerHistoryError = "Replay request could not be loaded: " + describeError(error);
        this.composerError = this.composerHistoryError;
      }
      return;
    }
    if (
      action !== this.historyActionRevision ||
      generation !== this.draftGeneration ||
      sourceRevision !== this.sourceRevision ||
      version !== this.editVersion ||
      !this.isConnected
    ) {
      return;
    }
    const focusOwned =
      paneRevision === this.paneRevision &&
      (root instanceof ShadowRoot ? root.activeElement : document.activeElement) === focusAtStart;
    if (this.composerDirty && !focusOwned) {
      this.composerHistoryError =
        "Replay request loading stopped. Choose Edit again when you are ready to replace this draft.";
      return;
    }
    await this.populateDraft(
      async () => {
        const request = saved.request,
          bodySource = request.bodySource;
        return {
          source: {
            method: request.method,
            url: request.url,
            headers: request.headers,
            body: request.bodyIsHex
              ? request.body
              : bytesToHex(encodeBody(request.body, "text", "utf-8")),
            bodyAvailable: true,
            bodyStreamed: bodySource?.kind === "captured",
            bodyBytes: saved.bodyBytes,
            notices: saved.notices,
          },
          origin: saved.source,
          capturedId: bodySource?.kind === "captured" ? bodySource.entryId : "",
          file: bodySource?.kind === "file" ? bodySource.path : "",
          sourceLabel: saved.source
            ? (saved.source.traceName ?? saved.source.traceId ?? "Original Traffic request") +
              " · " +
              (saved.source.originalId ?? saved.source.entryId)
            : "",
        };
      },
      "Loading the replay request…",
      "Replay request loaded. Review it before sending.",
      "Replay request",
      focusOwned,
    );
  }
  async showHistorySource(id: string): Promise<void> {
    const action = ++this.historyActionRevision,
      paneRevision = this.paneRevision,
      root = this.getRootNode() as Document | ShadowRoot,
      focusAtStart = root.activeElement;
    try {
      await invoke("session_detail", { id });
      if (
        action === this.historyActionRevision &&
        paneRevision === this.paneRevision &&
        root.activeElement === focusAtStart &&
        this.view === "composer" &&
        this.isConnected
      ) {
        this.$emit("source-traffic-request", id);
      }
    } catch {
      if (action === this.historyActionRevision && this.isConnected && this.view === "composer") {
        this.composerHistoryError =
          "This source entry is no longer in Traffic. Its trace and original entry association remain in history.";
      }
    }
  }
  returnToTraffic(): void {
    this.activateView("traffic");
  }
  private async checkSource(): Promise<void> {
    const id = this.composerSourceId;
    try {
      await invoke("session_detail", { id });
      if (this.composerSourceId === id) {
        this.composerSourceAvailable = true;
      }
    } catch {
      if (this.composerSourceId === id) {
        this.composerSourceAvailable = false;
      }
    }
  }
  async showComposerSource(): Promise<void> {
    this.composerSourcePopover.hidePopover();
    this.composerSourceButton.focus({ preventScroll: true });
    const id = this.composerSourceId,
      action = ++this.historyActionRevision,
      sourceRevision = this.sourceRevision,
      paneRevision = this.paneRevision,
      root = this.getRootNode() as Document | ShadowRoot,
      focusAtStart = root.activeElement;
    await this.checkSource();
    if (
      id === this.composerSourceId &&
      this.composerSourceAvailable &&
      action === this.historyActionRevision &&
      sourceRevision === this.sourceRevision &&
      paneRevision === this.paneRevision &&
      root.activeElement === focusAtStart &&
      this.view === "composer" &&
      this.isConnected
    ) {
      this.$emit("source-traffic-request", id);
    }
  }
  resolveComposerReplacement(replace: boolean): void {
    this.replaceComposerDialog.close();
    this.leaveResolve?.(replace);
    this.leaveResolve = null;
  }
  replacementCancelled(event: Event): void {
    event.preventDefault();
    this.resolveComposerReplacement(false);
  }
  async populateRequest(detail: SessionDetail): Promise<void> {
    await this.populateDraft(
      async () => ({
        source: await invoke<ComposerSource>("composer_source", { id: detail.id }),
        origin: { entryId: detail.id, traceId: null, originalId: null },
        capturedId: detail.id,
        file: "",
      }),
      "Loading the complete captured request…",
      "Captured request loaded. Review it before sending.",
    );
  }
  private async populateDraft(
    load: () => Promise<{
      source: ComposerSource;
      origin: ComposerOrigin | null;
      capturedId: string;
      file: string;
      sourceLabel?: string;
    }>,
    loading: string,
    loaded: string,
    requestKind = "Captured request",
    focusOwned = true,
  ): Promise<void> {
    if (this.leaveResolve) {
      return;
    }
    const sourceRevision = ++this.sourceRevision;
    if (this.composerDirty) {
      const replace = await new Promise<boolean>((resolve) => {
        this.leaveResolve = resolve;
        this.replaceComposerDialog.showModal();
      });
      if (!replace || sourceRevision !== this.sourceRevision) {
        return;
      }
    }
    const generation = ++this.draftGeneration;
    this.historyActionRevision++;
    this.draftFeedback = false;
    this.composerText = loading;
    this.composerError = "";
    const revision = ++this.bodyRevision,
      editVersion = this.editVersion,
      paneRevision = this.paneRevision;
    const root = this.getRootNode(),
      focusAtStart = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
    try {
      const { source, origin, capturedId, file, sourceLabel } = await load();
      if (
        generation !== this.draftGeneration ||
        revision !== this.bodyRevision ||
        editVersion !== this.editVersion ||
        !this.isConnected
      ) {
        if (generation === this.draftGeneration && this.isConnected) {
          this.composerText = requestKind + " loading stopped. Your current draft is preserved.";
        }
        return;
      }
      const revealRequest =
        focusOwned &&
        paneRevision === this.paneRevision &&
        (root instanceof ShadowRoot ? root.activeElement : document.activeElement) === focusAtStart;
      this.draftIdentity++;
      this.input("method").value = source.method;
      this.input("url").value = source.url;
      this.composerUrl = source.url;
      // Apply the complete draft before yielding to the editors' focus restoration.
      const queryRendered = this.queryEditor.replaceValue(source.url);
      this.validateUrlLength();
      const headersRendered = this.headerEditor.replaceValue(
        source.headers.map((header) => header.name + ": " + header.value).join("\n"),
      );
      this.capturedBodyId = capturedId;
      this.capturedBodyAvailable = source.bodyAvailable && !!capturedId;
      this.composerBodyFile = file;
      this.composerBodySizeLabel =
        source.bodyBytes == null
          ? "Complete encoded body; size will be measured before sending."
          : source.bodyBytes.toLocaleString() + " encoded bytes";
      this.composerBodyMode = file ? "file" : source.bodyStreamed ? "captured" : "inline";
      this.input("bodyMode").value = this.composerBodyMode;
      this.inlineBodyAvailable = source.bodyAvailable && !source.bodyStreamed;
      this.composerBodyMissing = !source.bodyAvailable;
      this.bodyTextEncoding = this.encodingForHeaders(source.headers);
      this.bodyViewBytes = null;
      this.input("body").value = source.body;
      this.bodyHexMode = true;
      this.setBodyBytes(encodeBody(source.body, "hex", this.bodyTextEncoding));
      this.input("nonIdempotent").checked = false;
      this.input("credentials").checked = false;
      this.composerSourceId = origin?.entryId ?? "";
      this.composerSourceLabel = origin
        ? (sourceLabel ?? source.method + " " + shortUrl(source.url))
        : "";
      this.composerSourceAvailable = !!origin;
      if (origin && sourceLabel != null) {
        void this.checkSource();
      }
      this.composerDirty = false;
      this.resultMatchesDraft = false;
      const appliedVersion = ++this.editVersion;
      if (revealRequest) {
        this.composerPane = "request";
      }
      this.updateRisk();
      await Promise.all([queryRendered, headersRendered]);
      if (revealRequest) {
        // The compact Request pane must be visible before its URL can receive focus.
        await this.$whenRendered();
      }
      if (
        generation !== this.draftGeneration ||
        revision !== this.bodyRevision ||
        appliedVersion !== this.editVersion ||
        !this.isConnected
      ) {
        if (generation === this.draftGeneration && this.isConnected) {
          this.composerText = requestKind + " loading stopped. Your current draft is preserved.";
        }
        return;
      }
      if (
        revealRequest &&
        paneRevision === this.paneRevision &&
        this.view === "composer" &&
        (root instanceof ShadowRoot ? root.activeElement : document.activeElement) === focusAtStart
      ) {
        this.input("url").focus();
      }
      this.composerText = loaded + (source.notices.length ? " " + source.notices.join(" ") : "");
      this.draftFeedback = true;
    } catch (error: unknown) {
      if (generation === this.draftGeneration && this.isConnected) {
        if (revision === this.bodyRevision && editVersion === this.editVersion) {
          this.composerError = requestKind + " could not be loaded: " + describeError(error);
          this.composerText = "Your previous draft is still available.";
        } else {
          this.composerText = requestKind + " loading stopped. Your current draft is preserved.";
        }
      }
    }
  }
  async executeComposer(event: Event): Promise<void> {
    event.preventDefault();
    if (this.composerBusy || this.composerBodyMissing) {
      return;
    }
    if (!this.composerForm.reportValidity()) {
      return;
    }
    if (!this.headerEditor.checkValidity()) {
      await this.headerEditor.reportValidity();
      return;
    }
    let body = "";
    let bodyIsHex = true;
    try {
      if (this.composerBodyMode === "inline") {
        const bytes = this.currentBodyBytes(),
          value = this.input("body").value;
        // Keep UTF-8 text within the existing text input budget. Other encodings,
        // normalized captured newlines and binary content use exact compact hex.
        bodyIsHex = this.bodyHexMode || !equalBodyBytes(encodeBody(value, "text", "utf-8"), bytes);
        body = bodyIsHex ? bytesToHex(bytes).replace(/\s/g, "") : value;
        if (body.length > 8 * 1024 * 1024) {
          throw new Error(
            "Body exceeds the inline request limit. Choose a replacement file instead.",
          );
        }
      }
    } catch (error: unknown) {
      this.composerBodyError = describeError(error);
      this.input("body").focus();
      return;
    }
    const generation = ++this.draftGeneration,
      sendRevision = ++this.sendRevision,
      draftIdentity = this.draftIdentity,
      paneRevision = this.paneRevision,
      messageRevision = this.messageRevision,
      resultRevision = ++this.resultRevision,
      operationId = crypto.randomUUID(),
      started = performance.now();
    this.historyActionRevision++;
    this.composerBusy = true;
    this.composerCancelBusy = false;
    this.cancelRequested = false;
    this.sendOperation = operationId;
    this.sendGeneration = generation;
    const updateElapsed = (): void => {
      if (this.sendOperation !== operationId || !this.composerBusy || !this.isConnected) {
        return;
      }
      this.composerElapsed = "Elapsed: " + this.elapsedLabel(performance.now() - started);
      this.elapsedTimer = window.setTimeout(updateElapsed, 200);
    };
    updateElapsed();
    this.composerError = "";
    this.draftFeedback = false;
    this.composerText = "Sending request…";
    const data = new FormData(this.composerForm);
    const version = this.editVersion;
    const label = String(data.get("method")) + " " + String(data.get("url"));
    let focusMoved = false;
    const root = this.getRootNode(),
      onFocus = (): void => {
        focusMoved = true;
      };
    // Disabling Send may blur its button. A later focusin transfers ownership.
    root.addEventListener("focusin", onFocus, true);
    document.addEventListener("focusin", onFocus, true);
    try {
      const result = await invoke<ComposerResult>("execute_composer", {
        operationId,
        request: {
          method: String(data.get("method") ?? ""),
          url: String(data.get("url") ?? ""),
          headers: this.headerEditor.getHeaders(),
          sourceEntryId: this.composerSourceId || null,
          bodySource:
            this.composerBodyMode === "captured"
              ? { kind: "captured", entryId: this.capturedBodyId }
              : this.composerBodyMode === "file"
                ? { kind: "file", path: this.composerBodyFile }
                : null,
          body,
          bodyIsHex,
          acknowledgeNonIdempotent: data.get("nonIdempotent") === "on",
          acknowledgeCredentials: data.get("credentials") === "on",
        },
      });
      if (
        !this.isConnected ||
        draftIdentity !== this.draftIdentity ||
        sendRevision !== this.sendRevision
      ) {
        return;
      }
      const matchesDraft = version === this.editVersion;
      if (resultRevision === this.resultRevision) {
        this.displayResult(
          result,
          label,
          matchesDraft,
          false,
          messageRevision === this.messageRevision,
        );
      }
      // A source acquisition can own progress without replacing the displayed
      // draft. Retain its send's response, but leave newer feedback and pane choices.
      if (generation === this.draftGeneration) {
        this.composerError = "";
        this.composerText = matchesDraft ? "Response received: HTTP " + result.status : "";
        this.draftFeedback = matchesDraft;
      }
      if (
        generation === this.draftGeneration &&
        resultRevision === this.resultRevision &&
        version === this.editVersion &&
        paneRevision === this.paneRevision &&
        !focusMoved &&
        this.view === "composer"
      ) {
        this.composerPane = "response";
      }
    } catch (error: unknown) {
      if (
        this.isConnected &&
        generation === this.draftGeneration &&
        sendRevision === this.sendRevision
      ) {
        if (this.cancelRequested && /cancel/i.test(describeError(error))) {
          this.composerError = "";
          this.composerText =
            "Request canceled. The server may already have received it. Your draft is unchanged.";
        } else {
          this.composerError = "Send failed: " + describeError(error);
          this.composerText =
            version === this.editVersion
              ? "The request draft is ready to retry."
              : "The earlier request failed. Your newer draft is unchanged.";
        }
      }
    } finally {
      root.removeEventListener("focusin", onFocus, true);
      document.removeEventListener("focusin", onFocus, true);
      if (sendRevision === this.sendRevision) {
        this.composerBusy = false;
        this.composerCancelBusy = false;
        this.sendOperation = "";
        this.composerElapsed = "";
        window.clearTimeout(this.elapsedTimer);
        if (this.historyVisible && this.isConnected) {
          void this.refreshHistory();
        }
      }
    }
  }
  disconnectedCallback(): void {
    this.menuPlacement.close();
    this.historyRevision++;
    this.draftGeneration++;
    this.sendRevision++;
    this.sourceRevision++;
    this.resultRevision++;
    this.historyActionRevision++;
    window.clearTimeout(this.elapsedTimer);
    this.leaveResolve?.(false);
    this.leaveResolve = null;
    super.disconnectedCallback();
  }
}
ComposerWorkspace.define("composer-workspace");
