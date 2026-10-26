import { WebUIElement, attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { describeError } from "../utilities.js";
import {
  selectedConverterText,
  restoreConverterSelection,
  focusConverterSource,
  type ConverterSelection,
} from "../converter-selection.js";
import { MeasuredMenu, menuAnchor, navigateMenu, copied } from "../context-menu.js";
import { copyDialog } from "../inspection-copy.js";

type Representation = "text" | "hex";
type Encoding = "utf8" | "utf16le" | "utf16be" | "windows1252";
interface Edit {
  input: string;
  representation: Representation;
  encoding: Encoding;
}
interface Draft extends Edit {
  operation: string;
  outputRepresentation: Representation;
  history: Edit[];
  future: Edit[];
  selectionStart: number;
  selectionEnd: number;
  scrollTop: number;
  outputScrollTop: number;
  theme: string;
}
interface Output {
  display: string;
  representation: Representation;
  textAvailable: boolean;
  byteLength: number;
  encoding: Encoding;
}
interface AuthConversion {
  id: string;
  label: string;
  hint: string;
  acceptsBareBase64: boolean;
}

export class ConverterHost extends WebUIElement {
  @attr({ mode: "boolean" }) standalone = false;
  @attr({ attribute: "data-theme" }) converterTheme = "system";
  @observable converterInput = "";
  @observable converterOperation = "";
  @observable authConversions: AuthConversion[] = [];
  @observable converterAuthHint = "";
  @observable converterRepresentation: Representation = "text";
  @observable converterEncoding: Encoding = "utf8";
  @observable converterOutputMode: Representation = "text";
  @observable converterOutput = "";
  @observable converterActualMode: Representation = "text";
  @observable converterTextAvailable = true;
  @observable converterReady = false;
  @observable converterBusy = false;
  @observable converterHandoff = false;
  @observable converterSaving = false;
  @observable converterSaveError = "";
  @observable converterError = "";
  @observable converterStatus = "";
  @observable converterMenuError = "";
  @observable converterMenuX = "0px";
  @observable converterMenuY = "0px";
  @observable converterEditable = false;
  @observable converterSelection = false;
  trigger!: HTMLButtonElement;
  panel!: HTMLElement;
  inputEditor!: HTMLTextAreaElement;
  outputEditor!: HTMLTextAreaElement;
  operationSelect!: HTMLSelectElement;
  selectionMenu!: HTMLElement;
  replaceDialog!: HTMLDialogElement;
  saveErrorDialog!: HTMLDialogElement;
  private history: Edit[] = [];
  private future: Edit[] = [];
  private revision = 0;
  private timer: number | undefined;
  private selection: ConverterSelection | null = null;
  private readonly menuPlacement = new MeasuredMenu(
    () => this.selectionMenu,
    (x, y) => {
      this.converterMenuX = x;
      this.converterMenuY = y;
      this.$flushUpdates();
    },
  );
  menuToggled(event: ToggleEvent): void {
    if (event.newState === "closed") {
      this.menuPlacement.close();
    }
  }
  private output: Output | null = null;
  private restoreFocus: HTMLElement | null = null;
  private resultInput: Record<string, unknown> | null = null;
  private pendingOutputScroll = 0;
  private active(): Element | null {
    let element: Element | null = document.activeElement;
    while (element?.shadowRoot?.activeElement) {
      element = element.shadowRoot.activeElement;
    }
    return element;
  }
  private readonly contextListener = (event: MouseEvent): void => {
    if (
      event.defaultPrevented ||
      event
        .composedPath()
        .some((node) => node instanceof HTMLElement && node.classList.contains("monaco-editor"))
    ) {
      return;
    }
    const selected = selectedConverterText(event);
    if (!selected || copyDialog(selected.source)) {
      return;
    }
    this.showContext(event, selected, "");
  };
  private readonly keyboardListener = (event: KeyboardEvent): void => {
    if (
      event.defaultPrevented ||
      event.repeat ||
      !(event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey))
    ) {
      return;
    }
    if (
      event
        .composedPath()
        .some((node) => node instanceof HTMLElement && node.classList.contains("monaco-editor"))
    ) {
      return;
    }
    const selection = selectedConverterText(event);
    if (selection && !copyDialog(selection.source)) {
      this.showContext(event, selection, "");
    }
  };
  private readonly requestListener = (event: Event): void => {
    const request = (event as CustomEvent<{ text: string; source: HTMLElement }>).detail;
    this.selection = { text: request.text, source: request.source };
    void this.acceptSelection();
  };
  private readonly headerListener = (event: Event): void => {
    const detail = (
      event as CustomEvent<{
        event: MouseEvent | KeyboardEvent;
        text: string;
        error: string;
        source: HTMLElement;
      }>
    ).detail;
    this.showContext(
      detail.event,
      selectedConverterText(detail.event) ?? { text: detail.text, source: detail.source },
      detail.error,
    );
  };
  protected hydratedCallback(): void {
    void this.loadAuthConversions();
    document.addEventListener("contextmenu", this.contextListener);
    document.addEventListener("keydown", this.keyboardListener);
    document.addEventListener("format-converter-request", this.requestListener);
    document.addEventListener("format-converter-header", this.headerListener);
    if (this.standalone) {
      void this.receiveDraft();
    }
  }
  private async loadAuthConversions(): Promise<void> {
    try {
      const catalog = await invoke<AuthConversion[]>("auth_conversion_catalog");
      if (!this.isConnected) {
        return;
      }
      this.authConversions = catalog;
      this.$flushUpdates();
      this.operationSelect.value = this.converterOperation;
      this.updateAuthHint();
    } catch {
      this.converterError =
        "Authentication conversions could not be loaded. Reopen the converter to retry.";
    }
  }
  private updateAuthHint(): void {
    this.converterAuthHint =
      this.authConversions.find((entry) => entry.id === this.converterOperation)?.hint ?? "";
  }
  disconnectedCallback(): void {
    this.menuPlacement.close();
    document.removeEventListener("contextmenu", this.contextListener);
    document.removeEventListener("keydown", this.keyboardListener);
    document.removeEventListener("format-converter-request", this.requestListener);
    document.removeEventListener("format-converter-header", this.headerListener);
    clearTimeout(this.timer);
    this.cancelWork();
    super.disconnectedCallback();
  }
  private async receiveDraft(): Promise<void> {
    try {
      const draft = await invoke<Draft>("take_converter_draft");
      this.applyDraft(draft);
      await invoke("acknowledge_converter_window");
      this.inputEditor.focus();
      this.schedule();
    } catch (error: unknown) {
      this.converterError = describeError(error);
    }
  }
  private edit(): Edit {
    return {
      input: this.converterInput,
      representation: this.converterRepresentation,
      encoding: this.converterEncoding,
    };
  }
  private remember(): void {
    this.history.push(this.edit());
    this.future = [];
    while (
      this.history.length > 20 ||
      this.history.reduce((size, edit) => size + edit.input.length, 0) > 8 * 1024 * 1024
    ) {
      this.history.shift();
    }
  }
  private draft(text?: string): Draft {
    const root = this.getRootNode();
    const theme =
      root instanceof ShadowRoot
        ? (root.host.getAttribute("data-theme") ?? "system")
        : this.converterTheme;
    return {
      ...this.edit(),
      ...(text !== undefined ? { input: text, representation: "text" as const } : {}),
      operation: this.converterOperation,
      outputRepresentation: this.converterOutputMode,
      history: text === undefined ? this.history : [],
      future: text === undefined ? this.future : [],
      selectionStart: this.inputEditor.selectionStart,
      selectionEnd: this.inputEditor.selectionEnd,
      scrollTop: this.inputEditor.scrollTop,
      outputScrollTop: this.outputEditor.scrollTop,
      theme,
    };
  }
  private applyDraft(draft: Draft): void {
    this.converterInput = draft.input;
    this.converterRepresentation = draft.representation;
    this.converterEncoding = draft.encoding;
    this.converterOperation = draft.operation;
    this.converterOutputMode = draft.outputRepresentation;
    this.history = draft.history ?? [];
    this.future = draft.future ?? [];
    this.converterTheme = draft.theme ?? "system";
    this.$flushUpdates();
    this.inputEditor.value = this.converterInput;
    this.operationSelect.value = this.converterOperation;
    this.updateAuthHint();
    this.inputEditor.setSelectionRange(draft.selectionStart ?? 0, draft.selectionEnd ?? 0);
    this.inputEditor.scrollTop = draft.scrollTop ?? 0;
    this.pendingOutputScroll = draft.outputScrollTop ?? 0;
  }
  toggle(): void {
    if (this.panel.matches(":popover-open")) {
      this.close();
    } else {
      if (!this.authConversions.length) {
        void this.loadAuthConversions();
      }
      this.restoreFocus = this.trigger;
      this.panel.showPopover();
      this.inputEditor.focus();
    }
  }
  private returnFocus(): void {
    if (
      (this.restoreFocus && this.panel.contains(this.restoreFocus)) ||
      !focusConverterSource(this.restoreFocus)
    ) {
      this.trigger.focus({ preventScroll: true });
    }
  }
  close(): void {
    if (this.standalone) {
      void invoke("close_converter_window").catch((error) => {
        this.converterError = describeError(error);
      });
      return;
    }
    this.panel.hidePopover();
    this.returnFocus();
  }
  panelKey(event: KeyboardEvent): void {
    if (event.key === "Escape" && !this.standalone) {
      event.preventDefault();
      event.stopPropagation();
      this.close();
    }
  }
  inputChanged(event: Event): void {
    this.remember();
    this.converterInput = (event.currentTarget as HTMLTextAreaElement).value;
    this.schedule();
  }
  changeRepresentation(event: Event): void {
    this.remember();
    this.converterRepresentation = (event.currentTarget as HTMLSelectElement)
      .value as Representation;
    this.schedule();
  }
  changeEncoding(event: Event): void {
    this.remember();
    this.converterEncoding = (event.currentTarget as HTMLSelectElement).value as Encoding;
    this.schedule();
  }
  changeOperation(event: Event): void {
    this.converterOperation = (event.currentTarget as HTMLSelectElement).value;
    this.updateAuthHint();
    this.schedule();
  }
  changeOutputMode(event: Event): void {
    this.converterOutputMode = (event.currentTarget as HTMLSelectElement).value as Representation;
    this.schedule();
  }
  inputKey(event: KeyboardEvent): void {
    if (
      !(event.ctrlKey || event.metaKey) ||
      event.altKey ||
      !["z", "y"].includes(event.key.toLowerCase())
    ) {
      return;
    }
    event.preventDefault();
    if (this.converterHandoff) {
      return;
    }
    const redo = event.key.toLowerCase() === "y" || event.shiftKey;
    const next = (redo ? this.future : this.history).pop();
    if (!next) {
      return;
    }
    (redo ? this.history : this.future).push(this.edit());
    this.converterInput = next.input;
    this.inputEditor.value = next.input;
    this.converterRepresentation = next.representation;
    this.converterEncoding = next.encoding;
    this.schedule();
  }
  private snapshot(): Record<string, unknown> {
    return {
      input: this.converterInput,
      representation: this.converterRepresentation,
      encoding: this.converterEncoding,
      operation: this.converterOperation,
      outputRepresentation: this.converterOutputMode,
    };
  }
  private cancelWork(): void {
    const revision = ++this.revision;
    void invoke("cancel_format_conversion", { revision }).catch(() => undefined);
  }
  private schedule(): void {
    clearTimeout(this.timer);
    this.cancelWork();
    this.converterReady = false;
    this.converterBusy = false;
    this.converterError = "";
    this.converterStatus = this.converterOperation ? "Waiting to convert…" : "Choose a conversion.";
    if (this.converterOperation) {
      this.timer = window.setTimeout(() => void this.run(), 180);
    }
  }
  cancel(): void {
    clearTimeout(this.timer);
    this.cancelWork();
    this.converterBusy = false;
    this.converterReady = false;
    this.converterStatus = "Conversion canceled. Input preserved.";
  }
  async run(): Promise<void> {
    clearTimeout(this.timer);
    if (!this.converterOperation || this.converterHandoff) {
      return;
    }
    const revision = ++this.revision,
      input = this.snapshot();
    this.converterReady = false;
    this.converterBusy = true;
    this.converterError = "";
    this.converterStatus = "Converting…";
    try {
      const result = await invoke<Output>("convert_format", { input, revision });
      if (revision !== this.revision || !this.isConnected) {
        return;
      }
      this.output = result;
      this.resultInput = input;
      this.converterOutput = result.display;
      this.outputEditor.value = result.display;
      this.outputEditor.scrollTop = this.pendingOutputScroll;
      this.pendingOutputScroll = 0;
      this.converterActualMode = result.representation;
      this.converterTextAvailable = result.textAvailable;
      this.converterReady = true;
      this.converterStatus =
        result.byteLength.toLocaleString() +
        " output bytes · " +
        (result.representation === "hex" ? "Hex" : "Text");
    } catch (error: unknown) {
      if (revision === this.revision) {
        this.converterError = describeError(error);
        this.converterStatus = "Input preserved. Correct it or try Convert again.";
      }
    } finally {
      if (revision === this.revision) {
        this.converterBusy = false;
      }
    }
  }
  chain(): void {
    if (!this.converterReady || !this.output) {
      return;
    }
    this.remember();
    this.converterInput = this.output.display;
    this.inputEditor.value = this.converterInput;
    this.converterRepresentation = this.output.representation;
    this.converterEncoding = this.output.encoding;
    this.schedule();
    this.inputEditor.focus();
  }
  async copyOutput(): Promise<void> {
    if (!this.converterReady) {
      return;
    }
    const revision = this.revision;
    try {
      await navigator.clipboard.writeText(this.converterOutput);
      if (this.isConnected && revision === this.revision) {
        this.converterStatus = "Output copied.";
      }
    } catch {
      if (this.isConnected && revision === this.revision) {
        this.converterError = "Output could not be copied. Select the output and use Copy.";
      }
    }
  }
  async save(): Promise<void> {
    if (!this.converterReady || !this.resultInput || this.converterSaving) {
      return;
    }
    this.converterSaving = true;
    const revision = this.revision,
      input = this.resultInput;
    try {
      const saved = await invoke<boolean>("save_converter_output", { input });
      if (revision === this.revision) {
        this.converterStatus = saved ? "Output saved." : "Save canceled.";
      }
    } catch (error: unknown) {
      this.converterSaveError = describeError(error);
      this.$flushUpdates();
      this.saveErrorDialog.showModal();
    } finally {
      this.converterSaving = false;
    }
  }
  async detach(): Promise<void> {
    if (this.converterHandoff) {
      return;
    }
    this.converterHandoff = true;
    clearTimeout(this.timer);
    this.cancelWork();
    this.converterBusy = false;
    try {
      await invoke("open_converter_window", { draft: this.draft() });
      if (!this.standalone) {
        const returnFocus = this.panel.matches(":popover-open");
        this.panel.hidePopover();
        this.converterInput = "";
        this.inputEditor.value = "";
        this.converterOutput = "";
        this.outputEditor.value = "";
        this.converterOperation = "";
        this.updateAuthHint();
        this.operationSelect.value = "";
        this.converterRepresentation = "text";
        this.converterEncoding = "utf8";
        this.converterOutputMode = "text";
        this.converterStatus = "";
        this.converterReady = false;
        this.history = [];
        this.future = [];
        if (returnFocus) {
          this.returnFocus();
        }
      }
    } catch (error: unknown) {
      this.converterError = describeError(error);
    } finally {
      this.converterHandoff = false;
    }
  }
  private showContext(
    event: MouseEvent | KeyboardEvent,
    selection: ConverterSelection,
    error: string,
  ): void {
    event.preventDefault();
    event.stopPropagation();
    this.selection = selection;
    const editable =
      selection.source instanceof HTMLTextAreaElement ||
      selection.source instanceof HTMLInputElement;
    this.converterEditable =
      (editable || selection.source.isContentEditable) &&
      !selection.source.hasAttribute("readonly") &&
      !selection.source.hasAttribute("disabled");
    this.converterSelection = selection.text.length > 0;
    this.converterMenuError = error;
    const anchor = menuAnchor(event, selection.source);
    this.$flushUpdates();
    this.selectionMenu.showPopover();
    this.menuPlacement.open(anchor);
    (this.selectionMenu.querySelector("button:not(:disabled)") as HTMLElement | null)?.focus();
  }
  menuKey(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      this.selectionMenu.hidePopover();
      this.restoreSource();
      return;
    }
    navigateMenu(
      event,
      [
        ...this.selectionMenu.querySelectorAll<HTMLButtonElement>(
          "button:not(:disabled):not([hidden])",
        ),
      ],
      this.shadowRoot?.activeElement ?? null,
    );
  }
  openSelection(): void {
    this.selectionMenu.hidePopover();
    void this.acceptSelection();
  }
  private async acceptSelection(): Promise<void> {
    if (!this.selection) {
      return;
    }
    if (this.converterHandoff) {
      this.converterError = "A converter window is opening. Try again when it is ready.";
      return;
    }
    if (this.standalone) {
      await this.selectionInWindow();
      return;
    }
    if (this.converterInput !== "") {
      this.replaceDialog.showModal();
      return;
    }
    this.replaceInput();
  }
  replaceInput(): void {
    if (!this.selection) {
      return;
    }
    this.replaceDialog.close();
    this.remember();
    this.converterInput = this.selection.text;
    this.inputEditor.value = this.converterInput;
    this.converterRepresentation = "text";
    this.restoreFocus = this.selection.source;
    this.panel.showPopover();
    this.inputEditor.focus();
    this.schedule();
  }
  private restoreSource(selection: ConverterSelection | null = this.selection): void {
    if (selection && this.panel.contains(selection.source) && !this.standalone) {
      this.panel.showPopover();
    }
    restoreConverterSelection(selection);
  }
  cancelReplacement(event: Event): void {
    event.preventDefault();
    this.replaceDialog.close();
    this.restoreSource();
  }
  closeSaveError(event: Event): void {
    event.preventDefault();
    this.saveErrorDialog.close();
    if (!this.standalone) {
      this.panel.showPopover();
    }
    this.outputEditor.focus();
  }
  async selectionInWindow(): Promise<void> {
    if (!this.selection || this.converterHandoff) {
      return;
    }
    const selection = this.selection;
    this.replaceDialog.close();
    const focus = this.active();
    this.converterHandoff = true;
    try {
      await invoke("open_converter_window", { draft: this.draft(selection.text) });
    } catch (error: unknown) {
      this.converterError = describeError(error);
      if (
        this.isConnected &&
        !this.standalone &&
        this.selection === selection &&
        this.active() === focus &&
        document.hasFocus()
      ) {
        this.panel.showPopover();
      }
    } finally {
      this.converterHandoff = false;
    }
  }
  private clipboardSelectionCurrent(selection: ConverterSelection): boolean {
    if (
      !this.isConnected ||
      document.hidden ||
      this.selection !== selection ||
      !selection.source.isConnected ||
      (!selection.source.getClientRects().length && !this.panel.contains(selection.source)) ||
      !this.selectionMenu.matches(":popover-open") ||
      !this.selectionMenu.contains(this.shadowRoot?.activeElement ?? null)
    ) {
      return false;
    }
    if (
      selection.source instanceof HTMLInputElement ||
      selection.source instanceof HTMLTextAreaElement
    ) {
      return selection.source.value === selection.inputValue;
    }
    return !selection.range || selection.range.toString() === selection.text;
  }
  async copySelection(): Promise<void> {
    if (!this.selection) {
      return;
    }
    const selection = this.selection;
    if (!this.clipboardSelectionCurrent(selection)) {
      return;
    }
    try {
      await navigator.clipboard.writeText(selection.text);
      if (!this.clipboardSelectionCurrent(selection)) {
        return;
      }
      this.selectionMenu.hidePopover();
      this.restoreSource(selection);
      copied(selection.source);
    } catch {
      if (this.clipboardSelectionCurrent(selection)) {
        this.converterMenuError = "Copy failed. Select the text and press Ctrl+C.";
      }
    }
  }
  async editSelection(action: string): Promise<void> {
    if (!this.selection || !this.converterEditable) {
      return;
    }
    const selection = this.selection;
    try {
      const text = action === "paste" ? await navigator.clipboard.readText() : "";
      if (action === "cut") {
        await navigator.clipboard.writeText(selection.text);
      }
      if (!this.clipboardSelectionCurrent(selection)) {
        return;
      }
      this.selectionMenu.hidePopover();
      this.restoreSource(selection);
      document.execCommand("insertText", false, text);
    } catch {
      if (this.clipboardSelectionCurrent(selection)) {
        this.converterMenuError = "Clipboard access failed. Use the editor keyboard shortcut.";
      }
    }
  }
}
ConverterHost.define("converter-host");
