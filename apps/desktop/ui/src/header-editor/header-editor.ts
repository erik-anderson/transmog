import { attr, observable, WebUIElement } from "@microsoft/webui-framework";
import {
  enabledHeaderFields,
  formatHeaderText,
  HeaderValidationError,
  mergeHeaderDraftRows,
  parseHeaderText,
  validateHeaderDraftRows,
  type HeaderDraftRow,
  type HeaderField,
} from "./headers.js";

export { HeaderValidationError, type HeaderField } from "./headers.js";

export interface HeaderChangeDetail {
  value: string;
  valid: boolean;
  error: string;
  headers: HeaderField[] | null;
}

/** Preserve row boundaries even when a partial row is not valid bulk text. */
export interface HeaderDraft {
  value: string;
  mode: "rows" | "text";
  rows: HeaderDraftRow[];
  customError: string;
}

interface HeaderRow extends HeaderDraftRow {
  id: string;
  number: number;
  nameId: string;
  valueId: string;
  enabledId: string;
  enabled: boolean;
  nameInvalid: boolean;
  valueInvalid: boolean;
}

let editorSequence = 0;

/** A lossless ordered header draft with structured and bulk text editing. */
export class HeaderEditor extends WebUIElement {
  @attr label = "Headers";
  @attr({ attribute: "readonly", mode: "boolean" }) readOnly = false;
  @attr({ mode: "boolean" }) disabled = false;
  /** Opt in only when the owning workspace retains HeaderDraft metadata. */
  @attr({ attribute: "allow-disable", mode: "boolean" }) allowDisable = false;
  @attr({ attribute: "maxlength" }) maxLength = "";
  @observable headerEditorRows: HeaderRow[] = [];
  @observable headerEditorMode = "rows";
  @observable headerEditorText = "";
  @observable headerEditorError = "";
  @observable headerEditorInvalid = false;
  @observable headerEditorErrorId = "";
  @observable headerEditorCanUndo = false;
  @observable headerEditorSuggestionsId = "";
  rawInput!: HTMLTextAreaElement;
  addButton!: HTMLButtonElement;
  modeButton!: HTMLButtonElement;
  private readonly editorId = `header-editor-${++editorSequence}`;
  private rowSequence = 0;
  private ready = false;
  private draft = "";
  private customError = "";
  private pendingDraft: HeaderDraft | null = null;
  private removed: { row: HeaderRow; index: number } | null = null;

  get value(): string {
    return this.draft;
  }

  set value(value: string) {
    if (value === this.draft) {
      return;
    }
    this.replaceValue(value);
  }

  /** Authoritative load/reset; unlike a binding echo, equal text replaces row boundaries. */
  replaceValue(value: string): void {
    this.draft = value;
    this.pendingDraft = null;
    if (this.ready) {
      const root = this.getRootNode() as Document | ShadowRoot,
        active = root.activeElement;
      const focusedRow = this.headerEditorRows.findIndex(
        (row) =>
          row.nameId === active?.id || row.valueId === active?.id || row.enabledId === active?.id,
      );
      const focusedValue =
        focusedRow >= 0 && this.headerEditorRows[focusedRow]!.valueId === active?.id;
      const focusedEnabled =
        focusedRow >= 0 && this.headerEditorRows[focusedRow]!.enabledId === active?.id;
      this.loadDraft();
      if (focusedRow >= 0 && active) {
        this.$flushUpdates();
        if (active.isConnected || (root.activeElement && root.activeElement !== document.body)) {
          return;
        }
        if (this.headerEditorMode === "text") {
          this.rawInput.focus({ preventScroll: true });
        } else {
          const row = this.headerEditorRows[Math.min(focusedRow, this.headerEditorRows.length - 1)];
          if (row) {
            root
              .getElementById(
                focusedEnabled ? row.enabledId : focusedValue ? row.valueId : row.nameId,
              )
              ?.focus({ preventScroll: true });
          } else {
            (this.readOnly ? this.modeButton : this.addButton).focus({ preventScroll: true });
          }
        }
      }
    }
  }

  protected hydratedCallback(): void {
    this.ready = true;
    this.headerEditorSuggestionsId = `${this.editorId}-suggestions`;
    this.headerEditorErrorId = `${this.editorId}-error`;
    if (this.pendingDraft) {
      this.restoreDraft(this.pendingDraft);
    } else {
      this.loadDraft();
    }
  }

  private row(header: HeaderDraftRow): HeaderRow {
    const id = `${this.editorId}-row-${++this.rowSequence}`;
    return {
      ...header,
      enabled: header.enabled !== false,
      id,
      number: 0,
      nameId: `${id}-name`,
      valueId: `${id}-value`,
      enabledId: `${id}-enabled`,
      nameInvalid: false,
      valueInvalid: false,
    };
  }

  private loadDraft(): void {
    this.headerEditorText = this.draft;
    this.removed = null;
    this.headerEditorCanUndo = false;
    this.customError = "";
    this.headerEditorRows = [];
    try {
      this.headerEditorRows = parseHeaderText(this.draft).map((header) => this.row(header));
    } catch {
      this.headerEditorMode = "text";
    }
    this.validate();
  }

  private fields(): HeaderField[] {
    return enabledHeaderFields(this.headerEditorRows);
  }

  private draftRows(): HeaderDraftRow[] {
    return this.headerEditorRows.map(({ name, value, enabled }) =>
      enabled ? { name, value } : { name, value, enabled: false },
    );
  }

  captureDraft(): HeaderDraft {
    if (this.pendingDraft) {
      return {
        ...this.pendingDraft,
        rows: this.pendingDraft.rows.map((header) => ({ ...header })),
      };
    }
    return {
      value: this.draft,
      mode: this.headerEditorMode === "text" ? "text" : "rows",
      rows: this.draftRows(),
      customError: this.customError,
    };
  }

  /** Restore without reparsing malformed rows into different, valid headers. */
  restoreDraft(draft: HeaderDraft): void {
    if (!this.ready) {
      this.pendingDraft = {
        ...draft,
        rows: draft.rows.map((header) => ({ ...header })),
      };
      this.draft = draft.value;
      return;
    }
    this.pendingDraft = null;
    this.draft = draft.value;
    this.customError = draft.customError;
    this.headerEditorMode = draft.mode;
    this.headerEditorText = draft.value;
    this.headerEditorRows = draft.rows.map((header) => this.row(header));
    this.removed = null;
    this.headerEditorCanUndo = false;
    this.validate();
  }

  getHeaders(): HeaderField[] {
    const limit = Number(this.maxLength);
    if (limit > 0 && this.draft.length > limit) {
      throw new HeaderValidationError(
        `Headers exceed the ${limit.toLocaleString()} character limit.`,
        1,
      );
    }
    const headers = this.headerEditorMode === "text" ? parseHeaderText(this.draft) : this.fields();
    if (this.headerEditorMode !== "text") {
      validateHeaderDraftRows(this.headerEditorRows);
    }
    if (this.customError) {
      throw new HeaderValidationError(this.customError, 1);
    }
    return headers;
  }

  /** Parent-specific constraints appear beside the same editing surface. */
  setCustomValidity(message: string): void {
    this.customError = message;
    if (this.ready) {
      this.validate();
    }
  }

  private validate(): HeaderChangeDetail {
    let headers: HeaderField[] | null = null;
    let invalid: HeaderValidationError | null = null;
    try {
      headers = this.getHeaders();
    } catch (error) {
      if (!(error instanceof HeaderValidationError)) {
        throw error;
      }
      invalid = error;
    }
    this.headerEditorError = invalid?.message ?? "";
    this.headerEditorInvalid = invalid !== null;
    this.headerEditorRows = this.headerEditorRows.map((row, index) => ({
      ...row,
      number: index + 1,
      nameInvalid: invalid?.line === index + 1 && invalid.field === "name",
      valueInvalid: invalid?.line === index + 1 && invalid.field === "value",
    }));
    return {
      value: this.draft,
      valid: invalid === null,
      error: this.headerEditorError,
      headers,
    };
  }

  reportValidity(): boolean {
    const result = this.validate();
    if (result.valid) {
      return true;
    }
    this.$flushUpdates();
    if (this.headerEditorMode === "text") {
      this.focusRawError();
    } else {
      const invalid = this.headerEditorRows.find((row) => row.nameInvalid || row.valueInvalid);
      if (invalid) {
        this.focusField(invalid.valueInvalid ? invalid.valueId : invalid.nameId);
      } else {
        this.addButton.focus();
      }
    }
    return false;
  }

  private focusField(id: string): void {
    const root = this.getRootNode() as Document | ShadowRoot;
    (root.getElementById(id) as HTMLElement | null)?.focus();
  }

  private focusRawError(): void {
    this.rawInput.focus();
    try {
      parseHeaderText(this.draft);
    } catch (error) {
      if (error instanceof HeaderValidationError) {
        const lines = this.draft.split("\n");
        const start = lines
          .slice(0, error.line - 1)
          .reduce((length, line) => length + line.length + 1, 0);
        this.rawInput.setSelectionRange(start, start + (lines[error.line - 1]?.length ?? 0));
      }
    }
  }

  editRow(id: string, field: "name" | "value", event: Event): void {
    if (this.readOnly || this.disabled) {
      return;
    }
    const value = (event.target as HTMLInputElement | HTMLTextAreaElement).value;
    this.headerEditorRows = this.headerEditorRows.map((row) =>
      row.id === id ? { ...row, [field]: value } : row,
    );
    this.changed(formatHeaderText(this.fields()));
  }

  editText(event: Event): void {
    if (!this.readOnly && !this.disabled) {
      this.removed = null;
      this.headerEditorCanUndo = false;
      this.changed((event.target as HTMLTextAreaElement).value);
    }
  }

  toggleHeader(id: string, event: Event): void {
    if (!this.allowDisable || this.readOnly || this.disabled) {
      return;
    }
    const enabled = (event.target as HTMLInputElement).checked;
    this.headerEditorRows = this.headerEditorRows.map((row) =>
      row.id === id ? { ...row, enabled } : row,
    );
    this.changed(formatHeaderText(this.fields()));
  }

  private changed(value: string): void {
    this.draft = value;
    this.headerEditorText = value;
    this.customError = "";
    this.$emit("header-change", this.validate());
  }

  addHeader(): void {
    if (this.readOnly || this.disabled) {
      return;
    }
    const row = this.row({ name: "", value: "" });
    this.headerEditorRows = [...this.headerEditorRows, row];
    this.changed(formatHeaderText(this.fields()));
    this.$flushUpdates();
    this.focusField(row.nameId);
  }

  removeHeader(id: string): void {
    if (this.readOnly || this.disabled) {
      return;
    }
    const index = this.headerEditorRows.findIndex((row) => row.id === id);
    const row = this.headerEditorRows[index];
    if (!row) {
      return;
    }
    this.removed = { row, index };
    this.headerEditorCanUndo = true;
    this.headerEditorRows = this.headerEditorRows.filter((item) => item.id !== id);
    this.changed(formatHeaderText(this.fields()));
    this.$flushUpdates();
    const next = this.headerEditorRows[Math.min(index, this.headerEditorRows.length - 1)];
    if (next) {
      this.focusField(next.nameId);
    } else {
      this.addButton.focus();
    }
  }

  undoRemove(): void {
    if (!this.removed || this.readOnly || this.disabled) {
      return;
    }
    const { row, index } = this.removed;
    const rows = [...this.headerEditorRows];
    rows.splice(index, 0, row);
    this.headerEditorRows = rows;
    this.removed = null;
    this.headerEditorCanUndo = false;
    this.changed(formatHeaderText(this.fields()));
    this.$flushUpdates();
    this.focusField(row.nameId);
  }

  toggleMode(): void {
    if (this.disabled) {
      return;
    }
    if (this.headerEditorMode === "rows") {
      try {
        validateHeaderDraftRows(this.headerEditorRows);
      } catch {
        this.reportValidity();
        return;
      }
      this.headerEditorMode = "text";
      this.validate();
      this.$flushUpdates();
      this.rawInput.focus();
      return;
    }
    try {
      this.headerEditorRows = mergeHeaderDraftRows(
        this.draftRows(),
        parseHeaderText(this.draft),
      ).map((header) => this.row(header));
    } catch {
      this.reportValidity();
      return;
    }
    this.headerEditorMode = "rows";
    this.validate();
    this.$flushUpdates();
    const first = this.headerEditorRows[0];
    if (first) {
      this.focusField(first.nameId);
    } else {
      (this.readOnly ? this.modeButton : this.addButton).focus();
    }
  }

  maxLengthChanged(): void {
    if (this.ready) {
      this.validate();
    }
  }

  preventValueNewline(event: KeyboardEvent): void {
    if (event.key === "Enter") {
      event.preventDefault();
    }
  }

  stopNative(event: Event): void {
    event.stopPropagation();
  }
}

HeaderEditor.define("header-editor");
