import { captureFocus, captureRenderScope } from "../rendering.js";
import { attr, observable, WebUIElement } from "@microsoft/webui-framework";
import {
  editQueryParameter,
  formatQueryUrl,
  newQueryParameter,
  parseQueryUrl,
  type QueryParameterField,
  type QueryUrlDraft,
} from "./query-parameters.js";

export interface QueryChangeDetail {
  value: string;
  rows: QueryParameterField[];
}

export type QueryEditorDraft = QueryChangeDetail;

interface QueryRow extends QueryParameterField {
  id: string;
  number: number;
  nameId: string;
  valueId: string;
  equalsId: string;
  equalsHintId: string;
}

let editorSequence = 0;

/** Optional structured URL editing; merely opening it never changes the URL. */
export class QueryEditor extends WebUIElement {
  @attr({ mode: "boolean" }) disabled = false;
  @observable queryEditorRows: QueryRow[] = [];
  @observable queryEditorCanUndo = false;
  addButton!: HTMLButtonElement;
  private readonly editorId = `query-editor-${++editorSequence}`;
  private rowSequence = 0;
  private draft: QueryUrlDraft = parseQueryUrl("");
  private ready = false;
  private removed: { row: QueryRow; index: number } | null = null;

  get value(): string {
    return formatQueryUrl(this.draft);
  }

  set value(value: string) {
    if (value !== this.value) {
      this.replaceValue(value);
    }
  }

  async replaceValue(value: string): Promise<void> {
    const focus = captureFocus(this);
    const root = this.getRootNode() as Document | ShadowRoot;
    const active = root.activeElement;
    const focused = this.queryEditorRows.findIndex((row) =>
      [row.nameId, row.valueId, row.equalsId].includes(active?.id ?? ""),
    );
    const previous = this.queryEditorRows[focused];
    const field =
      active?.id === previous?.valueId
        ? "valueId"
        : active?.id === previous?.equalsId
          ? "equalsId"
          : "nameId";
    this.draft = parseQueryUrl(value);
    if (this.ready) {
      this.loadRows();
    }
    if (this.ready && focused >= 0 && active) {
      const current = this.renderScope();
      await this.$whenRendered();
      if (current() && !active.isConnected && focus()) {
        const row = this.queryEditorRows[Math.min(focused, this.queryEditorRows.length - 1)];
        if (row) {
          this.focusField(field === "equalsId" && row.value ? row.valueId : row[field], false);
        } else {
          this.addButton.focus({ preventScroll: true });
        }
      }
    }
  }

  protected hydratedCallback(): void {
    this.ready = true;
    this.loadRows();
  }

  private row(field: QueryParameterField): QueryRow {
    const id = `${this.editorId}-row-${++this.rowSequence}`;
    return {
      ...field,
      id,
      number: 0,
      nameId: `${id}-name`,
      valueId: `${id}-value`,
      equalsId: `${id}-equals`,
      equalsHintId: `${id}-equals-hint`,
    };
  }

  private fields(): QueryParameterField[] {
    return this.queryEditorRows.map(({ name, value, hasEquals, rawName, rawValue }) => ({
      name,
      value,
      hasEquals,
      rawName,
      rawValue,
    }));
  }

  captureDraft(): QueryEditorDraft {
    return {
      value: this.value,
      rows: this.ready ? this.fields() : this.draft.rows.map((row) => ({ ...row })),
    };
  }

  restoreDraft(draft: QueryEditorDraft): void {
    this.draft = { ...parseQueryUrl(draft.value), rows: draft.rows.map((row) => ({ ...row })) };
    if (this.ready) {
      this.loadRows();
    }
  }

  private loadRows(): void {
    this.queryEditorRows = this.draft.rows.map((field, index) => ({
      ...this.row(field),
      number: index + 1,
    }));
    this.removed = null;
    this.queryEditorCanUndo = false;
  }

  private changed(): void {
    this.queryEditorRows = this.queryEditorRows.map((row, index) => ({
      ...row,
      number: index + 1,
    }));
    this.draft = {
      ...this.draft,
      rows: this.fields(),
      hasQuery: this.queryEditorRows.length > 0,
    };
    this.$emit("query-change", this.captureDraft());
  }

  editRow(id: string, field: "name" | "value", event: Event): void {
    if (this.disabled) {
      return;
    }
    const value = (event.target as HTMLInputElement | HTMLTextAreaElement).value;
    const previous = this.queryEditorRows.find((row) => row.id === id);
    if (!previous || previous[field] === value) {
      return;
    }
    this.queryEditorRows = this.queryEditorRows.map((row) =>
      row.id === id ? { ...row, ...editQueryParameter(row, field, value) } : row,
    );
    this.changed();
  }

  toggleEquals(id: string, event: Event): void {
    if (this.disabled) {
      return;
    }
    const hasEquals = (event.target as HTMLInputElement).checked;
    this.queryEditorRows = this.queryEditorRows.map((row) =>
      row.id === id && !row.value ? { ...row, hasEquals } : row,
    );
    this.changed();
  }

  async addParameter(): Promise<void> {
    const focus = captureFocus(this);
    if (this.disabled) {
      return;
    }
    const row = this.row(newQueryParameter());
    this.queryEditorRows = [...this.queryEditorRows, row];
    this.changed();
    const current = this.renderScope();
    await this.$whenRendered();
    if (!current() || !focus()) {
      return;
    }
    this.focusField(row.nameId);
  }

  async removeParameter(id: string): Promise<void> {
    const focus = captureFocus(this);
    if (this.disabled) {
      return;
    }
    const index = this.queryEditorRows.findIndex((row) => row.id === id);
    const row = this.queryEditorRows[index];
    if (!row) {
      return;
    }
    this.removed = { row, index };
    this.queryEditorCanUndo = true;
    this.queryEditorRows = this.queryEditorRows.filter((item) => item.id !== id);
    this.changed();
    const current = this.renderScope();
    await this.$whenRendered();
    if (!current() || !focus()) {
      return;
    }
    const next = this.queryEditorRows[Math.min(index, this.queryEditorRows.length - 1)];
    if (next) {
      this.focusField(next.nameId);
    } else {
      this.addButton.focus();
    }
  }

  async undoRemove(): Promise<void> {
    const focus = captureFocus(this);
    if (this.disabled || !this.removed) {
      return;
    }
    const { row, index } = this.removed;
    const rows = [...this.queryEditorRows];
    rows.splice(index, 0, row);
    this.queryEditorRows = rows;
    this.removed = null;
    this.queryEditorCanUndo = false;
    this.changed();
    const current = this.renderScope();
    await this.$whenRendered();
    if (!current() || !focus()) {
      return;
    }
    this.focusField(row.nameId);
  }
  private renderScope(): () => boolean {
    return captureRenderScope(this, () => [this.queryEditorRows, this.draft, this.disabled]);
  }

  private focusField(id: string, reveal = true): void {
    const root = this.getRootNode() as Document | ShadowRoot;
    const field = root.getElementById(id);
    field?.focus({ preventScroll: true });
    if (reveal) {
      field?.scrollIntoView({ block: "nearest", inline: "nearest" });
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

QueryEditor.define("query-editor");
