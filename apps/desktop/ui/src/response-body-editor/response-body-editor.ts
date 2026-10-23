import { WebUIElement, attr, observable } from "@microsoft/webui-framework";
import type * as Monaco from "monaco-editor/editor/editor.api";
import { loadMonaco, waitForStyles } from "../editor-runtime.js";
import { describeError } from "../utilities.js";
import {
  bodyByteLength,
  encodeBody,
  renderBodyBytes,
  type BodyMode,
} from "../response-body-codec.js";

export interface BodyCommit {
  value: string;
  save: boolean;
  mode: BodyMode;
  bytes?: number[];
}
export interface BodyEditorOptions {
  mode?: BodyMode;
  textEncoding?: string | null;
  bytes?: number[];
  saveUnavailableReason?: string;
  saveLabel?: string;
  saveDescription?: string;
}
export type BodyLanguage = "plaintext" | "html" | "svg" | "css" | "javascript" | "json";
let modelSequence = 0;

export function responseBodyLanguage(mediaType: string, value: string): BodyLanguage {
  const mime = mediaType.split(";")[0]!.trim().toLowerCase();
  if (mime === "image/svg+xml") {
    return "svg";
  }
  if (["text/html", "application/xhtml+xml"].includes(mime)) {
    return "html";
  }
  if (mime === "text/css") {
    return "css";
  }
  if (/(?:java|ecma)script/.test(mime)) {
    return "javascript";
  }
  if (mime === "application/json" || mime.endsWith("+json")) {
    return "json";
  }
  const start = value.trimStart().slice(0, 4096);
  if (/^(?:<\?xml\b[^?]*\?>\s*)?(?:<!--[\s\S]*?-->\s*)*<svg\b/i.test(start)) {
    return "svg";
  }
  if (/^<!doctype\s+html\b|^<(?:html|head|body|div|p|span|script|style)\b/i.test(start)) {
    return "html";
  }
  if (/^(?:[.#][\w-]+|body|html|:root)\s*\{[^}]*[\w-]+\s*:/s.test(start)) {
    return "css";
  }
  if (
    /^(?:["']use strict["'];|(?:export\s+)?(?:const|let|var|function|class)\b|import\s)/.test(start)
  ) {
    return "javascript";
  }
  if (/^\{\s*"[^"\n]+"\s*:|^\[\s*(?:\{|\[|"|\d|true|false|null|\])/.test(start)) {
    return "json";
  }
  return "plaintext";
}

/** One lazily allocated syntax editor for the active response-body dialog. */
export class ResponseBodyEditor extends WebUIElement {
  @attr theme = "system";
  @observable bodyLanguage: BodyLanguage = "plaintext";
  @observable bodyMode: BodyMode = "text";
  @observable bodyValueLabel = "Response body text";
  @observable bodyByteStatus = "";
  @observable bodySaveUnavailableReason = "";
  @observable bodyEditorReady = false;
  @observable bodyEditorLoaded = false;
  @observable bodyCanSave = false;
  @observable bodySaveLabel = "Save rule";
  @observable bodySaveDescription = "Save rule saves all pending rule changes.";
  @observable bodyBusy = false;
  @observable bodyFormatting = false;
  @observable bodyError = "";
  @observable bodyEditorStatus = "";
  surface!: HTMLDialogElement;
  fallbackInput!: HTMLTextAreaElement;
  editorHost!: HTMLDivElement;
  overflowHost!: HTMLDivElement;
  bodyEditorStyles!: HTMLLinkElement;
  languageSelect!: HTMLSelectElement;
  modeSelect!: HTMLSelectElement;
  private editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  private model: Monaco.editor.ITextModel | null = null;
  private monaco: typeof Monaco | null = null;
  private generation = 0;
  private opener: HTMLElement | null = null;
  private textEncoding: string | null = "utf-8";
  private contentListener: Monaco.IDisposable | null = null;
  private viewBytes: number[] | null = null;
  private viewValue = "";
  private systemTheme = matchMedia("(prefers-color-scheme: dark)");
  private highContrast = matchMedia("(forced-colors: active)");
  private themeListener = () => this.themeChanged();
  private dismissEditorPopup = (event: KeyboardEvent): void => {
    if (event.key !== "Escape" || !this.editor) {
      return;
    }
    // Monaco's widgets live outside its editor node. Prevent the native dialog's
    // Escape action until the visible editor popup has handled the key.
    if (
      Array.from(this.overflowHost.querySelectorAll(".monaco-hover")).some(
        (widget) =>
          widget.getClientRects().length && getComputedStyle(widget).visibility !== "hidden",
      )
    ) {
      event.preventDefault();
      this.editor.trigger("keyboard", "editor.action.hideHover", {});
      this.editor.focus();
    }
  };

  protected hydratedCallback(): void {
    this.systemTheme.addEventListener("change", this.themeListener);
    this.highContrast.addEventListener("change", this.themeListener);
    this.surface.addEventListener("keydown", this.dismissEditorPopup, true);
  }
  open(
    value: string,
    language: BodyLanguage,
    canSave: boolean,
    opener: HTMLElement,
    options: BodyEditorOptions = {},
  ): void {
    this.disposeEditor();
    const generation = ++this.generation;
    this.opener = opener;
    this.bodyLanguage = language;
    this.bodyMode = options.mode ?? "text";
    this.textEncoding = options.textEncoding === undefined ? "utf-8" : options.textEncoding;
    this.bodySaveLabel = options.saveLabel ?? "Save rule";
    this.bodySaveDescription =
      options.saveDescription ?? "Save rule saves all pending rule changes.";
    this.bodyCanSave = canSave;
    this.bodyBusy = false;
    this.bodyFormatting = false;
    this.bodyError = "";
    this.bodySaveUnavailableReason =
      options.saveUnavailableReason ??
      "Apply to the draft, then save the rule from its properties.";
    this.bodyValueLabel =
      this.bodyMode === "hex" ? "Response body hex bytes" : "Response body text";
    this.bodyEditorStatus = "Loading body editor…";
    this.$flushUpdates();
    this.fallbackInput.value = value;
    this.languageSelect.value = language;
    this.modeSelect.value = this.bodyMode;
    this.viewValue = this.fallbackInput.value;
    this.viewBytes = options.bytes ?? null;
    if (!this.viewBytes) {
      try {
        this.viewBytes = encodeBody(value, this.bodyMode, this.textEncoding);
      } catch {
        /* Invalid drafts remain editable. */
      }
    }
    this.updateByteStatus();
    this.surface.showModal();
    this.fallbackInput.focus();
    void this.ensureEditor(generation);
  }
  private async ensureEditor(generation: number): Promise<void> {
    try {
      this.monaco = (await loadMonaco()).monaco;
      if (generation !== this.generation || !this.isConnected || !this.surface.open) {
        return;
      }
      this.bodyEditorLoaded = true;
      this.$flushUpdates();
      await waitForStyles(this.bodyEditorStyles);
      if (generation !== this.generation || !this.isConnected || !this.surface.open) {
        return;
      }
      const focusFallback =
        this.getRootNode() instanceof ShadowRoot
          ? (this.getRootNode() as ShadowRoot).activeElement === this.fallbackInput
          : document.activeElement === this.fallbackInput;
      let bytes: number[] | null = null;
      try {
        bytes = this.currentBytes();
      } catch {
        /* Keep invalid draft text intact while hydrating. */
      }
      this.model = this.monaco.editor.createModel(
        this.fallbackInput.value,
        this.modelLanguage(),
        this.monaco.Uri.parse(`file:///transmog-responses/editor-${++modelSequence}/body`),
      );
      this.viewBytes = bytes;
      this.viewValue = this.model.getValue();
      this.bodyEditorReady = true;
      this.$flushUpdates();
      this.editor = this.monaco.editor.create(this.editorHost, {
        model: this.model,
        readOnly: this.bodyBusy,
        domReadOnly: this.bodyBusy,
        automaticLayout: true,
        minimap: { enabled: false },
        accessibilitySupport: "on",
        ariaLabel: this.bodyValueLabel,
        tabFocusMode: true,
        theme: this.editorTheme(),
        wordWrap: "on",
        fixedOverflowWidgets: true,
        overflowWidgetsDomNode: this.overflowHost,
      });
      this.contentListener = this.model.onDidChangeContent(() => this.updateByteStatus());
      this.updateEditorStatus();
      if (focusFallback) {
        this.editor.focus();
      }
    } catch (error: unknown) {
      if (generation !== this.generation) {
        return;
      }
      this.disposeEditor();
      this.bodyEditorLoaded = false;
      this.bodyEditorStatus =
        this.bodyMode === "hex"
          ? "Syntax editor could not load. You can still edit hex bytes."
          : "Syntax editor could not load. You can still edit plain text.";
      this.bodyError = describeError(error);
      this.$flushUpdates();
    }
  }
  changeLanguage(event: Event): void {
    const language = (event.currentTarget as HTMLSelectElement).value as BodyLanguage;
    if (
      this.bodyBusy ||
      this.bodyFormatting ||
      !["plaintext", "html", "svg", "css", "javascript", "json"].includes(language)
    ) {
      return;
    }
    this.bodyLanguage = language;
    if (this.model) {
      this.monaco?.editor.setModelLanguage(this.model, this.modelLanguage());
    }
    this.updateEditorStatus();
  }
  changeMode(event: Event): void {
    const mode = (event.currentTarget as HTMLSelectElement).value as BodyMode;
    if (
      this.bodyBusy ||
      this.bodyFormatting ||
      !["text", "hex"].includes(mode) ||
      mode === this.bodyMode
    ) {
      this.modeSelect.value = this.bodyMode;
      return;
    }
    try {
      const bytes = this.currentBytes(),
        value = renderBodyBytes(bytes, mode, this.textEncoding);
      this.bodyMode = mode;
      this.bodyValueLabel = mode === "hex" ? "Response body hex bytes" : "Response body text";
      this.bodyError = "";
      // Representation changes are not text edits: keeping the old text undo stack
      // would let Undo place hex digits in Text mode (and silently change bytes).
      this.viewValue = value;
      this.viewBytes = bytes;
      if (this.model) {
        this.model.setValue(value);
        this.monaco?.editor.setModelLanguage(this.model, this.modelLanguage());
        this.viewValue = this.model.getValue();
      }
      this.fallbackInput.value = value;
      if (!this.model) {
        this.viewValue = this.fallbackInput.value;
      }
      this.editor?.updateOptions({ ariaLabel: this.bodyValueLabel });
      this.updateEditorStatus();
      this.updateByteStatus();
    } catch (error: unknown) {
      this.modeSelect.value = this.bodyMode;
      this.bodyError = describeError(error);
    }
  }
  private modelLanguage(): string {
    return this.bodyMode === "hex"
      ? "plaintext"
      : this.bodyLanguage === "svg"
        ? "html"
        : this.bodyLanguage;
  }
  private updateEditorStatus(): void {
    this.bodyEditorStatus =
      this.bodyMode === "hex"
        ? "Edit complete byte pairs (00–FF), separated by spaces or new lines."
        : this.bodyLanguage === "svg"
          ? "SVG formatting is unavailable to preserve markup."
          : "Language affects highlighting and formatting. Content type is set in Response.";
  }
  updateByteStatus(): void {
    try {
      const value = this.model?.getValue() ?? this.fallbackInput.value,
        count =
          this.viewBytes && value === this.viewValue
            ? this.viewBytes.length
            : bodyByteLength(value, this.bodyMode, this.textEncoding);
      this.bodyByteStatus = `${count.toLocaleString()} ${count === 1 ? "byte" : "bytes"}`;
    } catch {
      this.bodyByteStatus =
        this.bodyMode === "hex"
          ? "Incomplete or invalid hex bytes"
          : "Text cannot be encoded without loss";
    }
  }
  private currentBytes(): number[] {
    const value = this.model?.getValue() ?? this.fallbackInput.value;
    return this.viewBytes && value === this.viewValue
      ? this.viewBytes
      : encodeBody(value, this.bodyMode, this.textEncoding);
  }
  async format(): Promise<void> {
    if (
      !this.editor ||
      this.bodyBusy ||
      this.bodyFormatting ||
      this.bodyMode === "hex" ||
      this.bodyLanguage === "plaintext" ||
      this.bodyLanguage === "svg"
    ) {
      return;
    }
    const root = this.getRootNode(),
      opener = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
    const generation = this.generation;
    this.bodyFormatting = true;
    this.bodyError = "";
    try {
      const action = this.editor.getAction("editor.action.formatDocument");
      if (!action) {
        throw new Error("Formatting is unavailable for this language.");
      }
      const deadline = Date.now() + 10000;
      while (!action.isSupported() && Date.now() < deadline) {
        await new Promise<void>((resolve) => setTimeout(resolve, 25));
        if (generation !== this.generation) {
          return;
        }
      }
      if (!action.isSupported()) {
        throw new Error("The formatter could not load. Try again.");
      }
      this.editor.pushUndoStop();
      await action.run();
      if (generation === this.generation) {
        this.editor.pushUndoStop();
        if ((root instanceof ShadowRoot ? root.activeElement : document.activeElement) === opener) {
          this.editor.focus();
        }
      }
    } catch (error: unknown) {
      if (generation === this.generation) {
        this.bodyError = "Content could not be formatted: " + describeError(error);
      }
    } finally {
      if (generation === this.generation) {
        this.bodyFormatting = false;
      }
    }
  }
  commit(save: boolean): void {
    if (this.bodyBusy || this.bodyFormatting || (save && !this.bodyCanSave)) {
      return;
    }
    const value = this.model?.getValue() ?? this.fallbackInput.value;
    let bytes: number[];
    try {
      bytes = this.currentBytes();
    } catch (error: unknown) {
      this.bodyError = describeError(error);
      return;
    }
    this.$emit("body-commit", { value, save, mode: this.bodyMode, bytes } satisfies BodyCommit);
  }
  keyboard(event: KeyboardEvent): void {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      if (this.bodyCanSave) {
        this.commit(true);
      }
    }
  }
  cancel(event?: Event): void {
    event?.preventDefault();
    if (!this.bodyBusy) {
      this.close();
    }
  }
  close(): void {
    if (this.surface.open) {
      this.surface.close();
    }
  }
  closed(): void {
    if (this.surface.open) {
      return;
    }
    this.generation++;
    this.disposeEditor();
    this.$emit("body-editor-closed");
    const opener = this.opener;
    this.opener = null;
    if (opener?.isConnected && opener.getClientRects().length) {
      opener.focus();
    }
  }
  private disposeEditor(): void {
    this.contentListener?.dispose();
    this.contentListener = null;
    this.editor?.dispose();
    this.editor = null;
    this.model?.dispose();
    this.model = null;
    this.bodyEditorReady = false;
  }
  private editorTheme(): string {
    const dark = this.theme === "dark" || (this.theme === "system" && this.systemTheme.matches);
    return this.highContrast.matches ? (dark ? "hc-black" : "hc-light") : dark ? "vs-dark" : "vs";
  }
  themeChanged(): void {
    this.monaco?.editor.setTheme(this.editorTheme());
  }
  bodyBusyChanged(): void {
    this.editor?.updateOptions({ readOnly: this.bodyBusy, domReadOnly: this.bodyBusy });
  }
  disconnectedCallback(): void {
    this.generation++;
    this.disposeEditor();
    this.systemTheme.removeEventListener("change", this.themeListener);
    this.highContrast.removeEventListener("change", this.themeListener);
    this.surface?.removeEventListener("keydown", this.dismissEditorPopup, true);
    super.disconnectedCallback();
  }
}
ResponseBodyEditor.define("response-body-editor");
