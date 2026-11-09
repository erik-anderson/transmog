import { observable, WebUIElement } from "@microsoft/webui-framework";
import type { LogValue } from "../script-diagnostics/models.js";
import { copyGesture, requestCopy, copyScalar } from "../inspection-copy.js";
import { selectedConverterText } from "../converter-selection.js";
interface SnapshotNode extends LogValue {
  named: boolean;
  label: string;
  index: number;
  open: boolean;
  children: SnapshotNode[];
}
export class ConsoleValues extends WebUIElement {
  @observable values: LogValue[] = [];
  @observable expanded: number[] = [];
  @observable nodes: SnapshotNode[] = [];
  valuesChanged(): void {
    this.updateNodes();
  }
  expandedChanged(): void {
    this.updateNodes();
  }
  private updateNodes(): void {
    const nodes = this.values.map(
      (value, index) =>
        ({
          ...value,
          index,
          named: value.parent !== null,
          label: value.name ? copyScalar(value.name) : '""',
          open: this.expanded.includes(index),
          children: [],
        }) as SnapshotNode,
    );
    const roots: SnapshotNode[] = [];
    for (const node of nodes) {
      const parent = node.parent == null ? undefined : nodes[node.parent];
      if (parent) {
        parent.children.push(node);
      } else {
        roots.push(node);
      }
    }
    this.nodes = roots;
  }
  toggle(node: SnapshotNode): void {
    this.dispatchEvent(
      new CustomEvent("snapshot-toggle", {
        bubbles: true,
        composed: true,
        detail: { index: node.index, open: !this.expanded.includes(node.index) },
      }),
    );
  }
  menu(node: SnapshotNode, event: MouseEvent | KeyboardEvent): void {
    if (!copyGesture(event) || selectedConverterText(event)) {
      return;
    }
    const source = event.currentTarget as HTMLElement;
    const values = JSON.stringify([this.values, this.expanded]);
    const lines = (item: SnapshotNode, depth = 0): string =>
      "  ".repeat(depth) +
      (item.named ? item.label + ": " : "") +
      copyScalar(item.display) +
      (item.truncated ? " … [Shortened]" : "") +
      (item.open ? item.children.map((child) => "\n" + lines(child, depth + 1)).join("") : "");
    requestCopy(event, {
      source,
      current: () => JSON.stringify([this.values, this.expanded]) === values,
      actions: [
        ...(node.named
          ? [{ id: "name", label: "Copy name", read: () => copyScalar(node.name) }]
          : []),
        node.children.length || node.kind === "object" || node.kind === "array"
          ? { id: "subtree", label: "Copy displayed subtree", read: () => lines(node) }
          : {
              id: "value",
              label: "Copy value",
              read: () => copyScalar(node.display) + (node.truncated ? " … [Shortened]" : ""),
            },
      ],
    });
  }
}
ConsoleValues.define("console-values");
