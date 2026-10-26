import { WebUIElement, attr, observable } from "@microsoft/webui-framework";
import type * as Monaco from "monaco-editor/editor/editor.api";
import { loadMonaco, waitForStyles } from "../editor-runtime.js";
import {
  boundedCopy,
  CopyError,
  copyGesture,
  requestCopy,
  type CopyAction,
} from "../inspection-copy.js";
import { copied } from "../context-menu.js";
import { selectedConverterText } from "../converter-selection.js";
import {
  SOURCE_EDITOR_BYTES,
  sourceLanguage,
  type BodySourcePreview,
  type SourceLanguage,
} from "./source.js";
import {
  formatJsonSource,
  jsonCopyActions,
  jsonNodeAt,
  jsonSnapshot,
  JsonSourceError,
  parseJsonSource,
  type JsonSourceDocument,
} from "./json-source.js";
import type { StructuredSnapshot } from "../extension-visualizer/models.js";

let modelSequence = 0;
const syntaxFailureNotice = "Syntax preview unavailable. Plain text remains available.";
const largePreviewNotice =
  "This larger preview is shown as plain text. Syntax highlighting, folding and Find are available for previews up to 1 MiB; lower Preview limit in Body options to use them.";
/** An inert, bounded source view. The host owns copying; Monaco owns navigation. */
export class BodySourceViewer extends WebUIElement {
  @attr({ mode: "boolean" }) active = false;
  @observable preview: BodySourcePreview | null = null;
  @observable bodySourceText = "";
  @observable sourceEditorReady = false;
  @observable sourceEditorLoaded = false;
  @observable sourceEditorLimited = false;
  @observable sourceWrap = true;
  @observable sourceStatus = "";
  @observable language = "auto";
  @observable sourceMode = "source";
  @observable sourceHasFields = false;
  @observable sourceSnapshot: StructuredSnapshot | null = null;
  private jsonDocument: JsonSourceDocument | null = null;
  private displayDocument: JsonSourceDocument | null = null;
  private jsonPrepared = false;
  private effectiveFormatted = false;
  private pendingReveal: string | null = null;
  private measuredText: string | null = null;
  private fallbackRevealRevision = 0;
  fallback!: HTMLPreElement;
  editorHost!: HTMLDivElement;
  overflowHost!: HTMLDivElement;
  sourceStyles!: HTMLLinkElement;
  private editor: Monaco.editor.IStandaloneCodeEditor | null = null;
  private model: Monaco.editor.ITextModel | null = null;
  private monaco: typeof Monaco | null = null;
  private generation = 0;
  private visible = false;
  private observer: IntersectionObserver | null = null;
  private savedView: Monaco.editor.ICodeEditorViewState | null = null;
  private loading = false;
  private deferred = false;
  private pendingFind = false;
  private readonly cancelFind = (): void => {
    this.pendingFind = false;
    this.pendingReveal = null;
  };
  private systemTheme = matchMedia("(prefers-color-scheme: dark)");
  private highContrast = matchMedia("(forced-colors: active)");
  private themeObserver: MutationObserver | null = null;
  private readonly themeListener = (): void => this.updateTheme();
  private readonly resume = (): void => {
    queueMicrotask(() => {
      if (this.deferred) {
        this.reconcile();
      }
    });
  };

  protected hydratedCallback(): void {
    this.editorHost.addEventListener("mousedown", this.pointerDown, true);
    this.editorHost.addEventListener("contextmenu", this.editorContext, true);
    this.editorHost.addEventListener("keydown", this.editorKey, true);
    this.editorHost.addEventListener("copy", this.copyEvent, true);
    this.systemTheme.addEventListener("change", this.themeListener);
    this.highContrast.addEventListener("change", this.themeListener);
    document.addEventListener("selectionchange", this.resume);
    document.addEventListener("focusin", this.resume);
    document.addEventListener("pointerdown", this.cancelFind, true);
    document.addEventListener("keydown", this.cancelFind, true);
    this.fallback.addEventListener("blur", this.resume);
    this.themeObserver = new MutationObserver(this.themeListener);
    const shell = this.getRootNode();
    if (shell instanceof ShadowRoot) {
      this.themeObserver.observe(shell.host, { attributes: true, attributeFilter: ["data-theme"] });
    }
    this.observer = new IntersectionObserver((entries) => {
      this.visible = entries.some((entry) => entry.isIntersecting);
      this.reconcile();
    });
    this.observer.observe(this);
  }
  activeChanged(): void {
    if (!this.active) {
      this.pendingFind = false;
      this.pendingReveal = null;
    }
    this.reconcile();
  }
  sourceModeChanged(): void {
    this.generation++;
    this.reconcile();
  }
  sourceHasFieldsChanged(): void {
    this.$emit("source-fields", this.sourceHasFields);
  }
  sourceEditorLimitedChanged(): void {
    this.$emit("source-limited", this.sourceEditorLimited);
  }
  previewChanged(): void {
    this.pendingFind = false;
    this.pendingReveal = null;
    this.releaseJson(true);
    this.generation++;
    this.disposeEditor();
    this.savedView = null;
    this.bodySourceText = this.preview?.formatted
      ? (this.preview.originalText ?? this.preview.text)
      : (this.preview?.text ?? "");
    this.sourceStatus = "";
    this.reconcile();
  }
  private reconcile(): void {
    if (!this.active || !this.visible || !this.preview || !this.isConnected) {
      this.generation++;
      this.disposeEditor();
      this.releaseJson();
      return;
    }
    this.prepareJson();
    if (this.measuredText !== this.bodySourceText) {
      this.measuredText = this.bodySourceText;
      this.sourceEditorLimited =
        this.bodySourceText.length > SOURCE_EDITOR_BYTES ||
        new TextEncoder().encode(this.bodySourceText).length > SOURCE_EDITOR_BYTES;
    }
    if (this.sourceMode === "fields" && this.jsonDocument) {
      if (this.editor || this.loading) {
        this.generation++;
        this.disposeEditor();
      }
      this.sourceSnapshot ??= jsonSnapshot(
        this.jsonDocument,
        "body-json-" + ++modelSequence,
        (id) => this.showJsonSource(id),
      );
      return;
    }
    this.sourceSnapshot = null;
    if (this.sourceEditorLimited) {
      this.generation++;
      this.disposeEditor();
      this.pendingFind = false;
      if (!this.sourceStatus.includes(largePreviewNotice)) {
        this.sourceStatus = [this.sourceStatus, largePreviewNotice].filter(Boolean).join("\n");
      }
      if (this.pendingReveal) {
        this.queueFallbackReveal();
      }
      return;
    }
    if (!this.editor && !this.loading) {
      void this.ensureEditor();
    }
  }
  private prepareJson(): void {
    if (this.jsonPrepared || !this.preview) {
      return;
    }
    const candidate =
      this.preview.formatted ||
      sourceLanguage(this.preview.mediaType, false) === "json" ||
      this.language === "json";
    if (!candidate) {
      return;
    }
    this.jsonPrepared = true;
    const source =
      this.preview.originalText ?? (!this.preview.formatted ? this.preview.text : null);
    if (source === null) {
      return;
    }
    try {
      const document = parseJsonSource(source);
      if (this.preview.formatted) {
        this.bodySourceText = formatJsonSource(document);
        this.effectiveFormatted = true;
      }
      this.jsonDocument = document;
      this.mapDisplay();
      this.sourceHasFields = true;
    } catch (error: unknown) {
      this.bodySourceText = source;
      this.sourceStatus =
        error instanceof JsonSourceError && error.reason === "limit"
          ? "JSON fields exceed the structured preview limit. Showing original text preview."
          : this.preview.truncated
            ? "JSON fields are unavailable for this incomplete preview. Source copying remains available."
            : "JSON fields are unavailable because this preview is not a complete JSON document. Source copying remains available.";
    }
  }
  private releaseJson(clearCapability = false): void {
    this.sourceSnapshot = null;
    this.jsonDocument = null;
    this.displayDocument = null;
    this.jsonPrepared = false;
    this.effectiveFormatted = false;
    if (clearCapability) {
      this.sourceHasFields = false;
    }
  }
  private mapDisplay(): void {
    this.displayDocument = null;
    if (!this.jsonDocument) {
      return;
    }
    try {
      const text = this.model?.getValue() ?? this.bodySourceText;
      const display = text === this.jsonDocument.source ? this.jsonDocument : parseJsonSource(text);
      if (display.nodes.size !== this.jsonDocument.nodes.size) {
        return;
      }
      for (const [id, node] of display.nodes) {
        const original = this.jsonDocument.nodes.get(id);
        if (
          !original ||
          node.kind !== original.kind ||
          node.name !== original.name ||
          node.scalar !== original.scalar ||
          node.children.length !== original.children.length
        ) {
          return;
        }
      }
      this.displayDocument = display;
    } catch {
      /* Source-only copying remains available when a display cannot map exactly. */
    }
  }
  private showJsonSource(id: string): void {
    this.pendingReveal = id === "$document" ? (this.jsonDocument?.root.id ?? null) : id;
    this.sourceMode = "source";
    this.$emit("source-mode", "source");
    this.reconcile();
  }
  private revealJson(): void {
    const node = this.pendingReveal && this.displayDocument?.nodes.get(this.pendingReveal);
    this.pendingReveal = null;
    if (!node || !this.editor || !this.model) {
      return;
    }
    const start = this.model.getPositionAt(node.start),
      end = this.model.getPositionAt(node.end);
    const range = {
      startLineNumber: start.lineNumber,
      startColumn: start.column,
      endLineNumber: end.lineNumber,
      endColumn: end.column,
    };
    this.editor.setSelection(range);
    this.editor.revealRangeInCenter(range);
    this.editor.focus();
  }
  private revealFallback(): void {
    const node = this.pendingReveal && this.displayDocument?.nodes.get(this.pendingReveal);
    this.pendingReveal = null;
    if (!node) {
      return;
    }
    const walker = document.createTreeWalker(this.fallback, NodeFilter.SHOW_TEXT),
      range = document.createRange();
    let offset = 0,
      startFound = false;
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      const end = offset + (text.textContent?.length ?? 0);
      if (!startFound && node.start <= end) {
        range.setStart(text, node.start - offset);
        startFound = true;
      }
      if (startFound && node.end <= end) {
        range.setEnd(text, node.end - offset);
        break;
      }
      offset = end;
    }
    if (!startFound) {
      return;
    }
    const root = this.getRootNode() as Document & { getSelection?: () => Selection | null },
      selection = root.getSelection?.() ?? document.getSelection();
    this.fallback.focus({ preventScroll: true });
    selection?.removeAllRanges();
    selection?.addRange(range);
    const rect = range.getBoundingClientRect(),
      viewport = this.fallback.getBoundingClientRect();
    this.fallback.scrollTop += rect.top - viewport.top;
    if (!this.sourceWrap) {
      this.fallback.scrollLeft += rect.left - viewport.left;
    }
  }
  private queueFallbackReveal(): void {
    const revision = ++this.fallbackRevealRevision,
      generation = this.generation,
      preview = this.preview,
      id = this.pendingReveal;
    const current = (): boolean =>
      revision === this.fallbackRevealRevision &&
      generation === this.generation &&
      this.preview === preview &&
      this.pendingReveal === id &&
      this.sourceMode === "source" &&
      this.active &&
      this.visible &&
      this.isConnected;
    // WebUI Changed callbacks precede the setter's DOM update notification.
    // Flush after the setter completes so the source is visible before focus,
    // range measurement and horizontal/vertical scrolling consume the intent.
    queueMicrotask(() => {
      if (!current()) {
        return;
      }
      this.$flushUpdates();
      if (current()) {
        this.revealFallback();
      }
    });
  }
  private async ensureEditor(): Promise<void> {
    const generation = ++this.generation;
    this.loading = true;
    const current = (): boolean =>
      generation === this.generation &&
      this.isConnected &&
      this.active &&
      this.visible &&
      !!this.preview;
    try {
      const runtime = await loadMonaco();
      if (!current()) {
        return;
      }
      this.monaco = runtime.monaco;
      this.sourceEditorLoaded = true;
      this.$flushUpdates();
      await waitForStyles(this.sourceStyles);
      if (!current()) {
        return;
      }
      // Preserve passive reading until focus and selection leave. A still-current
      // explicit Find or Show in source request takes precedence over old reading;
      // later input cancels that intent before this asynchronous continuation.
      const root = this.getRootNode(),
        focused = root instanceof ShadowRoot ? root.activeElement : document.activeElement;
      const selection =
        (root as Document & { getSelection?: () => Selection | null }).getSelection?.() ??
        document.getSelection();
      this.deferred =
        !this.pendingFind &&
        !this.pendingReveal &&
        (focused === this.fallback ||
          (!!selection &&
            !selection.isCollapsed &&
            !!selection.rangeCount &&
            selection.getRangeAt(0).intersectsNode(this.fallback)));
      if (this.deferred) {
        return;
      }
      const fallbackScroll = this.fallback.scrollTop;
      this.model = this.monaco.editor.createModel(
        this.bodySourceText,
        this.currentLanguage(),
        this.monaco.Uri.parse(`inmemory://transmog-body/preview-${++modelSequence}`),
      );
      // A model-wide LF convention also keeps an originally recognized LF XSSI
      // first line recognizable when the remaining source mostly uses CRLF.
      // Whole-preview copies still retain the original string's line endings.
      this.model.setEOL(this.monaco.editor.EndOfLineSequence.LF);
      this.mapDisplay();
      // Monaco normalizes model line endings. Whole-preview copies use the source
      // string, while selections read the model's plain-text range.
      this.sourceEditorReady = true;
      this.$flushUpdates();
      this.editor = this.monaco.editor.create(this.editorHost, {
        model: this.model,
        readOnly: true,
        domReadOnly: true,
        contextmenu: false,
        emptySelectionClipboard: false,
        automaticLayout: true,
        minimap: { enabled: false },
        lineNumbers: "on",
        folding: true,
        wordWrap: this.sourceWrap ? "on" : "off",
        accessibilitySupport: "on",
        ariaLabel: "Body source preview",
        fontSize: 12,
        lineHeight: 19,
        fontFamily: '"Cascadia Mono",Consolas,monospace',
        tabFocusMode: true,
        renderValidationDecorations: "off",
        quickSuggestions: false,
        suggestOnTriggerCharacters: false,
        parameterHints: { enabled: false },
        inlineSuggest: { enabled: false },
        links: false,
        colorDecorators: false,
        occurrencesHighlight: "off",
        selectionHighlight: false,
        codeLens: false,
        fixedOverflowWidgets: true,
        overflowWidgetsDomNode: this.overflowHost,
        theme: this.editorTheme(),
      });
      if (this.savedView) {
        this.editor.restoreViewState(this.savedView);
      } else {
        this.editor.setScrollTop(fallbackScroll);
      }
      if (this.pendingFind) {
        this.find();
      }
      if (this.pendingReveal) {
        this.revealJson();
      }
      if (this.sourceStatus === syntaxFailureNotice) {
        this.sourceStatus = "";
      }
    } catch {
      if (current()) {
        this.disposeEditor();
        this.sourceEditorLoaded = false;
        this.sourceStatus = syntaxFailureNotice;
        this.mapDisplay();
        this.$flushUpdates();
        if (this.pendingReveal) {
          this.queueFallbackReveal();
        }
      }
    } finally {
      this.loading = false;
      // Visibility can change while the shared runtime or stylesheet is pending.
      if (generation !== this.generation) {
        this.reconcile();
      }
    }
  }
  languageChanged(): void {
    if (!this.jsonPrepared) {
      this.reconcile();
    }
    if (this.model) {
      this.monaco?.editor.setModelLanguage(this.model, this.currentLanguage());
    }
  }
  private currentLanguage(): SourceLanguage {
    return this.language === "auto"
      ? sourceLanguage(this.preview?.mediaType ?? null, !!this.preview?.formatted)
      : (this.language as SourceLanguage);
  }
  sourceWrapChanged(): void {
    this.editor?.updateOptions({ wordWrap: this.sourceWrap ? "on" : "off" });
  }
  sourceEditorReadyChanged(): void {
    this.$emit("source-ready", this.sourceEditorReady);
  }
  find(): void {
    if (this.sourceEditorLimited) {
      this.sourceStatus = largePreviewNotice;
      return;
    }
    this.pendingFind = !this.editor;
    if (!this.editor) {
      this.reconcile();
      return;
    }
    this.editor?.focus();
    this.editor?.trigger("toolbar", "actions.find", null);
  }
  private readonly pointerDown = (event: MouseEvent): void => {
    // Monaco's right-button mousedown would move its selection before the menu.
    if (event.button === 2 && this.sourceEvent(event)) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  };
  private readonly editorKey = (event: KeyboardEvent): void => {
    if (!this.sourceEvent(event)) {
      return;
    }
    if (copyGesture(event)) {
      this.editorContext(event);
    } else if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "c" &&
      this.editor?.getSelection()?.isEmpty()
    ) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
  };
  private readonly copyEvent = (event: ClipboardEvent): void => {
    if (!this.sourceEvent(event)) {
      return;
    }
    event.preventDefault();
    event.stopImmediatePropagation();
    const selection = this.editor?.getSelection();
    if (!selection || selection.isEmpty() || !this.model || !event.clipboardData) {
      return;
    }
    try {
      event.clipboardData.setData("text/plain", boundedCopy(this.model.getValueInRange(selection)));
      copied(this.editorHost);
    } catch (error: unknown) {
      this.sourceStatus =
        error instanceof CopyError ? error.message : "Copy failed. The clipboard is unavailable.";
    }
  };
  private readonly editorContext = (event: MouseEvent | KeyboardEvent): void => {
    if (!this.sourceEvent(event) || !copyGesture(event) || !this.editor || !this.model) {
      return;
    }
    const editor = this.editor,
      model = this.model,
      selection = editor.getSelection();
    const pointer =
      event instanceof MouseEvent
        ? editor.getTargetAtClientPoint(event.clientX, event.clientY)?.position
        : null;
    const selected =
      selection &&
      !selection.isEmpty() &&
      (!(event instanceof MouseEvent) || (!!pointer && selection.containsPosition(pointer)))
        ? model.getValueInRange(selection)
        : "";
    const source = editor.getDomNode()?.querySelector("textarea") ?? this.editorHost;
    const targetPosition =
      event instanceof MouseEvent
        ? pointer
        : selection && !selection.isEmpty()
          ? selection.getStartPosition()
          : editor.getPosition();
    const targetNode =
      this.displayDocument && targetPosition
        ? jsonNodeAt(this.displayDocument, model.getOffsetAt(targetPosition))
        : null;
    const position = editor.getPosition(),
      caret = position && editor.getScrolledVisiblePosition(position),
      rect = this.editorHost.getBoundingClientRect();
    this.openCopy(event, source, selected, {
      current: () => this.editor === editor && this.model === model,
      restoreFocus: () => editor.focus(),
      ...(targetNode ? { semanticId: targetNode.id } : {}),
      ...(event instanceof KeyboardEvent && caret
        ? { anchor: { x: rect.left + caret.left, y: rect.top + caret.top + caret.height } }
        : {}),
    });
  };
  private sourceEvent(event: Event): boolean {
    const target = event
      .composedPath()
      .find((item): item is HTMLElement => item instanceof HTMLElement);
    if (
      !target ||
      target.closest(".find-widget,.suggest-widget,.monaco-hover,.parameter-hints-widget")
    ) {
      return false;
    }
    const input = target.closest("input,textarea,button,select,[contenteditable=true]");
    return !input || input.matches("textarea.inputarea");
  }
  fallbackContext(event: MouseEvent | KeyboardEvent): void {
    if (!copyGesture(event)) {
      return;
    }
    const selection = selectedConverterText(event);
    let semanticId: string | undefined;
    if (event instanceof KeyboardEvent && selection?.range && this.displayDocument) {
      const prefix = selection.range.cloneRange();
      prefix.selectNodeContents(this.fallback);
      prefix.setEnd(selection.range.startContainer, selection.range.startOffset);
      semanticId = jsonNodeAt(this.displayDocument, prefix.toString().length)?.id;
    }
    this.openCopy(event, this.fallback, selection?.text ?? "", semanticId ? { semanticId } : {});
  }
  fallbackKey(event: KeyboardEvent): void {
    if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "f" &&
      this.sourceEditorLimited
    ) {
      event.preventDefault();
      event.stopPropagation();
      this.sourceStatus = largePreviewNotice;
    } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "a") {
      event.preventDefault();
      event.stopPropagation();
      const range = document.createRange();
      range.selectNodeContents(this.fallback);
      const root = this.getRootNode() as Document & { getSelection?: () => Selection | null },
        selection = root.getSelection?.() ?? document.getSelection();
      selection?.removeAllRanges();
      selection?.addRange(range);
    } else {
      this.fallbackContext(event);
    }
  }
  fallbackCopy(event: ClipboardEvent): void {
    event.preventDefault();
    event.stopPropagation();
    const root = this.getRootNode() as Document & { getSelection?: () => Selection | null },
      selection = root.getSelection?.() ?? document.getSelection();
    if (!selection || selection.isCollapsed || !event.clipboardData) {
      return;
    }
    try {
      event.clipboardData.setData("text/plain", boundedCopy(selection.toString()));
      copied(this.fallback);
    } catch (error: unknown) {
      this.sourceStatus =
        error instanceof CopyError ? error.message : "Copy failed. The clipboard is unavailable.";
    }
  }
  private openCopy(
    event: MouseEvent | KeyboardEvent,
    source: HTMLElement,
    selected: string,
    options: {
      current?: () => boolean;
      restoreFocus?: () => void;
      anchor?: { x: number; y: number };
      semanticId?: string;
    },
  ): void {
    const preview = this.preview;
    if (!preview) {
      return;
    }
    const actions: CopyAction[] = [];
    if (selected) {
      actions.push({ id: "selection", label: "Copy", read: () => selected });
    }
    const text = this.bodySourceText,
      document = this.jsonDocument;
    actions.push({ id: "preview", label: "Copy body preview", read: () => text });
    if (this.effectiveFormatted && preview.originalText !== null) {
      actions.push({
        id: "original",
        label: "Copy original text preview",
        read: () => preview.originalText!,
      });
    }
    const node = options.semanticId && document?.nodes.get(options.semanticId);
    if (node && document) {
      actions.push(...jsonCopyActions(document, node));
    }
    if (selected) {
      actions.push({
        id: "converter",
        label: "Open in Format converter",
        convert: true,
        read: () => selected,
      });
    }
    requestCopy(event, {
      source,
      actions,
      current: () =>
        this.isConnected &&
        this.active &&
        this.visible &&
        this.preview === preview &&
        this.bodySourceText === text &&
        (options.current?.() ?? true),
      ...(options.restoreFocus ? { restoreFocus: options.restoreFocus } : {}),
      ...(options.anchor ? { anchor: options.anchor } : {}),
    });
  }
  private editorTheme(): string {
    const root = this.getRootNode(),
      configured = root instanceof ShadowRoot ? root.host.getAttribute("data-theme") : null;
    const dark = configured === "dark" || (configured !== "light" && this.systemTheme.matches);
    return this.highContrast.matches ? (dark ? "hc-black" : "hc-light") : dark ? "vs-dark" : "vs";
  }
  private updateTheme(): void {
    if (this.editor) {
      this.monaco?.editor.setTheme(this.editorTheme());
    }
  }
  private disposeEditor(): void {
    if (this.editor) {
      this.savedView = this.editor.saveViewState();
    }
    this.editor?.dispose();
    this.editor = null;
    this.model?.dispose();
    this.model = null;
    this.sourceEditorReady = false;
  }
  disconnectedCallback(): void {
    this.generation++;
    this.disposeEditor();
    this.releaseJson();
    this.observer?.disconnect();
    this.observer = null;
    this.themeObserver?.disconnect();
    this.themeObserver = null;
    this.systemTheme.removeEventListener("change", this.themeListener);
    this.highContrast.removeEventListener("change", this.themeListener);
    document.removeEventListener("selectionchange", this.resume);
    document.removeEventListener("focusin", this.resume);
    document.removeEventListener("pointerdown", this.cancelFind, true);
    document.removeEventListener("keydown", this.cancelFind, true);
    this.fallback?.removeEventListener("blur", this.resume);
    this.editorHost?.removeEventListener("mousedown", this.pointerDown, true);
    this.editorHost?.removeEventListener("contextmenu", this.editorContext, true);
    this.editorHost?.removeEventListener("keydown", this.editorKey, true);
    this.editorHost?.removeEventListener("copy", this.copyEvent, true);
    super.disconnectedCallback();
  }
}
BodySourceViewer.define("body-source-viewer");
