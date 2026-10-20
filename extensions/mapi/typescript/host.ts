/** The extension's consumed subset of the host's version-one wire contract. */
import type { Direction } from "./model.js";

export interface Header {
  readonly index: number;
  readonly name: string;
  readonly value?: readonly number[] | null;
  readonly redacted: boolean;
}

export interface BodyReference {
  readonly sourceId: string;
  readonly length: string;
  readonly availability: string;
}

export interface Message {
  readonly evidenceId: string;
  readonly headers: readonly Header[];
  readonly body?: BodyReference | null;
  readonly method?: string;
  readonly path?: string;
  readonly status?: number;
}

export interface VisualizationInput {
  readonly instanceId: string;
  readonly exchangeId: string;
  readonly requestNumber: string;
  readonly generation: string;
  readonly direction: Direction;
  readonly request?: Message | null;
  readonly response?: Message | null;
}

export interface Diagnostic {
  readonly code: string;
  readonly message: string;
}

export interface SourceSpan {
  readonly sourceId: string;
  readonly offset: string;
  readonly length: string;
}

export type HostValue =
  | { kind: "string" | "integer" | "guid" | "timestamp"; value: string }
  | { kind: "boolean"; value: boolean }
  | { kind: "float"; value: number }
  | { kind: "enum"; value: { name: string; raw: string } }
  | { kind: "flags"; value: { names: string[]; raw: string } }
  | { kind: "object" | "bytes" };

export interface TreeNode {
  id: string;
  name: string;
  children: TreeNode[];
  value?: HostValue;
  spans?: SourceSpan[];
  childrenToken?: string;
  presentation?: "tree" | "table" | "text" | "hex";
  format: {
    protocol?: string;
    byteOrder?: string;
    charset?: string;
    units?: string;
    encoding?: string;
    radix?: number;
  };
}

export interface HostSource {
  readonly id: string;
  readonly name: string;
  readonly bytesBase64: string;
  readonly parents: readonly SourceSpan[];
  readonly transform: string;
  readonly exactMapping: boolean;
}

export type ParseStatus = "complete" | "partial" | "malformed" | "limited" | "unavailable";

export interface VisualizationDocument {
  readonly title: string;
  readonly status: ParseStatus;
  readonly updates?: "final";
  readonly presentation: "tree";
  readonly summary?: string;
  readonly roots: TreeNode[];
  readonly sources?: HostSource[];
  readonly diagnostics?: readonly Diagnostic[];
}

export interface DocumentStep {
  readonly kind: "document";
  readonly document: VisualizationDocument;
}

export type VisualizationStep =
  | DocumentStep
  | {
      readonly kind: "needInput";
      readonly token: string;
      readonly reads: readonly { sourceId: string; offset: string; maxBytes: number }[];
    }
  | { readonly kind: "needDecoded"; readonly token: string; readonly sourceId: string }
  | {
      readonly kind: "needContext";
      readonly token: string;
      readonly request: { position: "before"; cursor: string | null; limit: number };
    };

export interface BytePage {
  readonly sourceId: string;
  readonly offset: string;
  readonly bytes: readonly number[];
}

export type DecodeContinuation =
  | { readonly kind: "ready"; readonly decoded: { readonly body: BodyReference } }
  | { readonly kind: "unavailable"; readonly reason: string };

export interface ContextPage {
  readonly exchanges: readonly { readonly input: VisualizationInput }[];
  readonly status: "more" | "complete" | "limited" | "unavailable";
  readonly nextCursor?: string | null;
}

export interface TrafficEvent {
  readonly exchangeId: string;
  readonly revision: string;
  readonly input: VisualizationInput;
}
