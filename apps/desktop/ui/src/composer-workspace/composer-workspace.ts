import initialState from "../initial-state.json";
import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import type { ComposerSource, ComposerResult, ComposerSnapshot, SessionDetail } from "../models.js";
import { describeError, responseTextEncoding, shortUrl } from "../utilities.js";
import type { CertificateViewer } from "../certificate-viewer/certificate-viewer.js";
import type { HeaderEditor, HeaderChangeDetail } from "../header-editor/header-editor.js";
import {
  type ResponseBodyEditor,
  responseBodyLanguage,
  type BodyCommit,
} from "../response-body-editor/response-body-editor.js";
import { bytesToHex, encodeBody, equalBodyBytes, renderBodyBytes } from "../response-body-codec.js";

export class ComposerWorkspace extends WorkspaceElement {
  certificateViewer!: CertificateViewer;
  showCertificates(event: Event): void {
    if (this.composerResult) {
      void this.certificateViewer.show(
        this.composerResult.detail.id,
        event.currentTarget as HTMLElement,
      );
    }
  }
  @attr view = "traffic";
  @attr theme = "system";
  @observable composerText = initialState.composerText;
  @observable composerError = "";
  @observable composerBusy = false;
  @observable composerDirty = false;
  @observable composerPane = "request";
  @observable composerResult: ComposerResult | null = null;
  @observable resultRequestLabel = "";
  @observable resultTone = "";
  @observable resultMatchesDraft = false;
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
  @observable composerHistory: Array<{
    id: number;
    label: string;
    status: string;
    sourceId: string;
    sourceLabel: string;
  }> = [];
  @observable composerHistoryBusy = false;
  @observable composerHistoryError = "";
  private historyVisible = false;
  private historyRevision = 0;
  private capturedBodyId = "";
  private inlineBodyAvailable = true;

  @observable composerSourceId = "";
  @observable composerSourceLabel = "";
  @observable composerSourceAvailable = false;
  composerForm!: HTMLFormElement;
  headerEditor!: HeaderEditor;
  bodyEditor!: ResponseBodyEditor;
  replaceComposerDialog!: HTMLDialogElement;
  private draftGeneration = 0;
  private draftIdentity = 0;
  private bodyRevision = 0;
  private editVersion = 0;
  private paneRevision = 0;
  private sendRevision = 0;
  private sourceRevision = 0;
  private bodyTextEncoding: string | null = "utf-8";
  private bodyViewValue = "";
  private bodyViewBytes: number[] | null = null;
  private bodyEditorDraft: { generation: number; revision: number; editVersion: number } | null =
    null;
  private leaveResolve: ((value: boolean) => void) | null = null;
  protected hydratedCallback(): void {
    this.updateRisk();
    this.updateBodyStatus();
  }
  viewChanged(): void {
    if (this.view === "composer" && this.composerSourceId) {
      void this.checkSource();
    }
  }
  private input(name: string): HTMLInputElement {
    return this.composerForm.elements.namedItem(name) as HTMLInputElement;
  }
  markComposerDirty(event: Event): void {
    // Header editor owns its native input events and emits one complete draft change.
    if (event.composedPath().includes(this.headerEditor)) {
      return;
    }
    const name = (event.target as HTMLInputElement).name;
    if (name === "bodyView") {
      return;
    }
    this.markDirty();
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
  private markDirty(): void {
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
  }
  showComposerPane(pane: string): void {
    this.paneRevision++;
    this.composerPane = pane;
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
  expandBody(event: Event): void {
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
    this.bodyEditor.open(
      value,
      responseBodyLanguage(mediaType, value),
      false,
      event.currentTarget as HTMLElement,
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
  async showHistorySource(id: string): Promise<void> {
    try {
      await invoke("session_detail", { id });
      this.$emit("source-traffic-request", id);
    } catch {
      this.composerHistoryError =
        "This source entry is no longer in Traffic. Its trace and original entry association remain in history.";
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
    const id = this.composerSourceId;
    await this.checkSource();
    if (id === this.composerSourceId && this.composerSourceAvailable) {
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
    this.composerText = "Loading the complete captured request…";
    this.composerError = "";
    const revision = ++this.bodyRevision,
      editVersion = this.editVersion;
    const root = this.getRootNode(),
      focusAtStart = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
    try {
      const source = await invoke<ComposerSource>("composer_source", { id: detail.id });
      if (
        generation !== this.draftGeneration ||
        revision !== this.bodyRevision ||
        editVersion !== this.editVersion ||
        !this.isConnected
      ) {
        if (generation === this.draftGeneration && this.isConnected) {
          this.composerText = "Captured request loading stopped. Your current draft is preserved.";
        }
        return;
      }
      this.draftIdentity++;
      this.input("method").value = source.method;
      this.input("url").value = source.url;
      this.headerEditor.replaceValue(
        source.headers.map((header) => header.name + ": " + header.value).join("\n"),
      );
      this.capturedBodyId = detail.id;
      this.capturedBodyAvailable = source.bodyAvailable;
      this.composerBodyFile = "";
      this.composerBodySizeLabel =
        source.bodyBytes == null
          ? "Complete encoded body; size will be measured before sending."
          : source.bodyBytes.toLocaleString() + " encoded bytes";
      this.composerBodyMode = source.bodyStreamed ? "captured" : "inline";
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
      this.composerSourceId = detail.id;
      this.composerSourceLabel = source.method + " " + shortUrl(source.url);
      this.composerSourceAvailable = true;
      this.composerDirty = false;
      this.resultMatchesDraft = false;
      this.editVersion++;
      this.composerPane = "request";
      this.updateRisk();
      if (
        this.view === "composer" &&
        (root instanceof ShadowRoot ? root.activeElement : document.activeElement) === focusAtStart
      ) {
        this.input("url").focus();
      }
      this.composerText =
        "Captured request loaded. Review it before sending." +
        (source.notices.length ? " " + source.notices.join(" ") : "");
    } catch (error: unknown) {
      if (generation === this.draftGeneration && this.isConnected) {
        if (revision === this.bodyRevision && editVersion === this.editVersion) {
          this.composerError = "Captured request could not be loaded: " + describeError(error);
          this.composerText = "Your previous draft is still available.";
        } else {
          this.composerText = "Captured request loading stopped. Your current draft is preserved.";
        }
      }
    }
  }
  async executeComposer(event: Event): Promise<void> {
    event.preventDefault();
    if (this.composerBusy || this.composerBodyMissing) {
      return;
    }
    if (!this.headerEditor.reportValidity()) {
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
      paneRevision = this.paneRevision;
    this.composerBusy = true;
    this.composerError = "";
    this.composerText = "Sending request…";
    const data = new FormData(this.composerForm);
    const version = this.editVersion;
    const label = String(data.get("method")) + " " + String(data.get("url"));
    try {
      const result = await invoke<ComposerResult>("execute_composer", {
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
      this.composerResult = result;
      this.resultRequestLabel = label;
      this.resultMatchesDraft = version === this.editVersion;
      this.resultTone =
        result.status >= 400 ? "failed" : result.status >= 300 ? "redirect" : "success";
      // A source acquisition can own progress without replacing the displayed
      // draft. Retain its send's response, but leave newer feedback and pane choices.
      if (generation === this.draftGeneration) {
        this.composerText =
          "Response received: HTTP " +
          result.status +
          (this.resultMatchesDraft ? "" : ". The request draft has newer edits.");
      }
      if (
        generation === this.draftGeneration &&
        version === this.editVersion &&
        paneRevision === this.paneRevision &&
        this.view === "composer"
      ) {
        this.composerPane = "response";
      }
      if (this.historyVisible) {
        void this.refreshHistory();
      }
    } catch (error: unknown) {
      if (
        this.isConnected &&
        generation === this.draftGeneration &&
        sendRevision === this.sendRevision
      ) {
        this.composerError = "Send failed: " + describeError(error);
        this.composerText =
          version === this.editVersion
            ? "The request draft is ready to retry."
            : "The earlier request failed. Your newer draft is unchanged.";
      }
    } finally {
      if (sendRevision === this.sendRevision) {
        this.composerBusy = false;
      }
    }
  }
  disconnectedCallback(): void {
    this.historyRevision++;
    this.draftGeneration++;
    this.sendRevision++;
    this.sourceRevision++;
    this.leaveResolve?.(false);
    this.leaveResolve = null;
    super.disconnectedCallback();
  }
}
ComposerWorkspace.define("composer-workspace");
