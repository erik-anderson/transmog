import { invoke } from '@tauri-apps/api/core';
import type { Lifecycle, ClientIdentity, SessionDetail, HeadView, StoredBodyMetadata } from './models.js';

export function lifecycleLabel(lifecycle: Lifecycle): string {
  switch (lifecycle) {
    case 'running': return 'Running';
    case 'stopping': return 'Stopping';
    case 'failed': return 'Needs attention';
    default: return 'Stopped';
  }
}

export function callerLabel(caller: ClientIdentity): string {
  if (caller.kind === 'remote') return 'Remote';
  if (caller.kind === 'local-unknown') return 'Local process unknown';
  const pid = caller.processId === null ? '' : ` · ${caller.processId}`;
  return `${caller.processName ?? 'Local process'}${pid}`;
}

export function describeError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  if (typeof error === 'object' && error !== null) {
    const message = (error as {message?: unknown}).message;
    if (typeof message === 'string') return message;
    try {
      return JSON.stringify(error).slice(0, 512);
    } catch {
      return 'unserializable failure';
    }
  }
  return 'unknown failure';
}

export function optionalText(value: FormDataEntryValue | null): string | null {
  const text = String(value ?? '').trim();
  return text.length === 0 ? null : text;
}

export function parseHex(value: string): number[] {
  const compact = value.replace(/\s+/g, '');
  if (compact.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(compact)) {
    throw new Error('body must be hexadecimal bytes');
  }
  const bytes: number[] = [];
  for (let index = 0; index < compact.length; index += 2) {
    bytes.push(Number.parseInt(compact.slice(index, index + 2), 16));
  }
  return bytes;
}

export function parseHeaderLines(value: string): Array<{name: string; value: string}> {
  return value.split(/\r?\n/).filter((line) => line.trim().length > 0).map((line) => {
    const separator = line.indexOf(':');
    if (separator <= 0) throw new Error('each header must use “Name: value”');
    const name = line.slice(0, separator).trim();
    const fieldValue = line.slice(separator + 1).trim();
    if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name)) throw new Error(`invalid HTTP header name: ${name}`);
    if (/[\x00-\x08\x0a-\x1f\x7f]/.test(fieldValue)) throw new Error(`invalid control character in header: ${name}`);
    return { name, value: fieldValue };
  });
}

export function encodeHeaders(headers: Array<{name: string; value: string}>): Array<{name: number[]; value: number[]}> {
  const encoder = new TextEncoder();
  return headers.map((header) => ({
    name: Array.from(encoder.encode(header.name)),
    value: Array.from(encoder.encode(header.value)),
  }));
}

export function shortUrl(value: string): string {
  try {
    const url = new URL(value);
    return `${url.host}${url.pathname}${url.search}`.slice(0, 96);
  } catch {
    return value.slice(0, 96);
  }
}

export function editableCharacterEncoding(declared: string | null): string | null {
  switch (declared?.trim().toLowerCase() ?? 'utf-8') {
    case 'utf-8': return 'utf-8';
    case 'us-ascii': return 'us-ascii';
    case 'utf-16':
    case 'utf-16le': return 'utf-16le';
    case 'utf-16be': return 'utf-16be';
    case 'utf-32':
    case 'utf-32le': return 'utf-32le';
    case 'utf-32be': return 'utf-32be';
    default: return null;
  }
}

export function encodeEditedText(value: string, encoding: string | null): number[] {
  if (encoding === null) throw new Error('the captured character encoding cannot be reproduced safely');
  if (encoding === 'us-ascii') {
    const bytes = Array.from(value, (character) => character.codePointAt(0) ?? 0);
    if (bytes.some((byte) => byte > 0x7f)) throw new Error('the edit contains characters that cannot be represented as US-ASCII');
    return bytes;
  }
  if (encoding === 'utf-8' || encoding === 'utf-8-bom') {
    const bytes = Array.from(new TextEncoder().encode(value));
    return encoding.endsWith('-bom') ? [0xef, 0xbb, 0xbf, ...bytes] : bytes;
  }
  if (encoding.startsWith('utf-16')) {
    const littleEndian = encoding.includes('le');
    const bytes: number[] = encoding.endsWith('-bom')
      ? (littleEndian ? [0xff, 0xfe] : [0xfe, 0xff])
      : [];
    for (let index = 0; index < value.length; index += 1) {
      const unit = value.charCodeAt(index);
      if (unit >= 0xd800 && unit <= 0xdbff) {
        const next = value.charCodeAt(index + 1);
        if (!(next >= 0xdc00 && next <= 0xdfff)) throw new Error('the edit contains an unpaired Unicode surrogate');
      } else if (unit >= 0xdc00 && unit <= 0xdfff && !(value.charCodeAt(index - 1) >= 0xd800 && value.charCodeAt(index - 1) <= 0xdbff)) {
        throw new Error('the edit contains an unpaired Unicode surrogate');
      }
      bytes.push(...(littleEndian ? [unit & 0xff, unit >>> 8] : [unit >>> 8, unit & 0xff]));
    }
    return bytes;
  }
  if (encoding.startsWith('utf-32')) {
    const littleEndian = encoding.includes('le');
    const bytes: number[] = encoding.endsWith('-bom')
      ? (littleEndian ? [0xff, 0xfe, 0x00, 0x00] : [0x00, 0x00, 0xfe, 0xff])
      : [];
    for (const character of value) {
      const scalar = character.codePointAt(0) ?? 0;
      if (scalar >= 0xd800 && scalar <= 0xdfff) throw new Error('the edit contains an unpaired Unicode surrogate');
      bytes.push(...(littleEndian
        ? [scalar & 0xff, (scalar >>> 8) & 0xff, (scalar >>> 16) & 0xff, scalar >>> 24]
        : [scalar >>> 24, (scalar >>> 16) & 0xff, (scalar >>> 8) & 0xff, scalar & 0xff]));
    }
    return bytes;
  }
  throw new Error(`unsupported captured character encoding: ${encoding}`);
}

export async function loadSessionDetail(sessionId: string, waitForCompletedBody: boolean): Promise<SessionDetail> {
  const attempts = waitForCompletedBody ? 20 : 1;
  let detail: SessionDetail | null = null;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    detail = await invoke<SessionDetail>('session_detail', { id: sessionId });
    const body = detail.storedBodies.find((candidate) => candidate.boundary === 'client-response');
    const bodyFinalized = body !== undefined && body.availability !== 'capturing';
    if (detail.terminal !== 'completed' || bodyFinalized) {
      return detail;
    }
    await new Promise((resolve) => window.setTimeout(resolve, 10));
  }
  return detail!;
}

export function clientResponseSource(detail: SessionDetail): {request: HeadView; response: HeadView; body: StoredBodyMetadata} | null {
  const request = detail.requests.find((head) => head.boundary === 'client-request');
  const response = detail.responses.find((head) => head.boundary === 'client-response');
  const body = detail.storedBodies.find((candidate) => candidate.boundary === 'client-response');
  return detail.terminal==='completed' && (detail.sequenceLoss??0)===0 && request?.method!==null && request?.target!==null && request !== undefined && response !== undefined && response.status!==null && response.status>=200 && response.status<=599 && body?.availability === 'complete'
    ? { request, response, body }
    : null;
}

export function autoResponseUnavailableReason(detail: SessionDetail): string {
  if(detail.terminal!=='completed')return 'Wait for this request to complete before saving its response.';
  if((detail.sequenceLoss??0)>0)return 'Some exchange evidence was lost; this response cannot be copied faithfully.';
  const status=detail.responses.find(head=>head.boundary==='client-response')?.status;
  if(status!==undefined && status!==null && (status<200 || status>599))return 'This response status cannot be replayed as a saved response.';
  if (!detail.requests.some((head) => head.boundary === 'client-request')) {
    return 'The original client request is unavailable.';
  }
  if (!detail.responses.some((head) => head.boundary === 'client-response')) {
    return 'The client-visible response has not completed.';
  }
  const body = detail.storedBodies.find((candidate) => candidate.boundary === 'client-response');
  if (body === undefined) return 'The client-visible response body metadata is unavailable.';
  if (body.availability !== 'complete') {
    return body.reason ?? `The client-visible response body is ${body.availability} and cannot be replayed exactly.`;
  }
  return 'The captured request or response is incomplete.';
}
