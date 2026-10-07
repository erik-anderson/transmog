import type { ColumnId, ColumnPreference, SessionSummary, WorkspacePreferences } from './models.js';
import initialState from './initial-state.json';

export const columnDefinitions: ReadonlyArray<{id: ColumnId; label: string; numeric: boolean; placeholder: string}> = [
  {id:'method', label:'Method', numeric:false, placeholder:'GET, POST…'},
  {id:'status', label:'Status', numeric:true, placeholder:'HTTP status, e.g. 304'},
  {id:'process', label:'Process / PID', numeric:false, placeholder:'Process name'},
  {id:'host', label:'Host', numeric:false, placeholder:'example.com'},
  {id:'path', label:'Path and query', numeric:false, placeholder:'/api/'},
  {id:'duration', label:'Duration', numeric:true, placeholder:'Milliseconds'},
  {id:'response-bytes', label:'Response size', numeric:true, placeholder:'Bytes'},
  {id:'protocol', label:'Protocol', numeric:false, placeholder:'HTTP/1.1'},
  {id:'request-bytes', label:'Request size', numeric:true, placeholder:'Bytes'},
  {id:'state', label:'State', numeric:false, placeholder:'active, completed, failed'},
  {id:'content-type', label:'Content type', numeric:false, placeholder:'application/json'},
  {id:'started-at', label:'Start time', numeric:true, placeholder:'Unix milliseconds'},
  {id:'pid', label:'Process ID', numeric:true, placeholder:'PID'},
  {id:'url', label:'Full URL', numeric:false, placeholder:'https://example.com/api/'},
];

export function defaultWorkspace(): WorkspacePreferences {
  return structuredClone(initialState.workspace) as WorkspacePreferences;
}

export function normalizeWorkspace(value: WorkspacePreferences | undefined): WorkspacePreferences {
  const fallback = defaultWorkspace();
  if (!value || value.columns?.length !== columnDefinitions.length) return fallback;
  const known = new Set(columnDefinitions.map((column) => column.id));
  if (new Set(value.columns.map((column) => column.id)).size !== known.size || value.columns.some((column) => !known.has(column.id))) return fallback;
  return { ...fallback, ...value, columns:value.columns.map((column) => ({...column,width:Math.max(32,Math.min(1200,column.width))})) };
}

export function displayColumns(columns: ColumnPreference[]) {
  let offset = 0;
  return [...columns.filter((column) => column.visible && column.pinned), ...columns.filter((column) => column.visible && !column.pinned)].map((column) => {
    const definition = columnDefinitions.find((definition) => definition.id === column.id)!;
    const result = {...column,...definition,widthCss:column.width+'px',offsetCss:offset+'px'};
    if (column.pinned) offset += column.width;
    return result;
  });
}

export function formatBytes(value: number): string {
  if (value < 1024) return value + ' B';
  if (value < 1024*1024) return (value/1024).toFixed(1) + ' KB';
  return (value/(1024*1024)).toFixed(1) + ' MB';
}

export function statusTone(row: SessionSummary): string {
  if (row.status === 304) return 'not-modified';
  if (row.terminal === 'failed') return 'failed';
  if (row.status === null) return 'pending';
  if (row.status >= 500) return 'failed';
  if (row.status >= 400) return 'warning';
  if (row.status >= 300) return 'redirect';
  return 'success';
}

export function cellText(row: SessionSummary, column: ColumnId): string {
  switch (column) {
    case 'method': return row.method;
    case 'status': return row.status === null ? 'Pending' : row.status === 304 ? '304 Not Modified' : String(row.status);
    case 'process': return row.caller.kind === 'remote' ? 'Remote' : (row.caller.processName ?? 'Unknown process') + (row.caller.processId === null ? '' : ' ('+row.caller.processId+')');
    case 'pid': return row.caller.processId?.toString() ?? '—';
    case 'host': return row.host;
    case 'path': return row.path;
    case 'url': return row.url ?? row.host+row.path;
    case 'protocol': return row.protocol === 'Http1' ? 'HTTP/1.1' : row.protocol === 'Http2' ? 'HTTP/2' : row.protocol;
    case 'duration': return row.durationMs+' ms';
    case 'response-bytes': return formatBytes(row.responseBytes);
    case 'request-bytes': return formatBytes(row.requestBytes);
    case 'state': return row.terminal === 'active' ? 'In progress' : row.terminal === 'completed' ? 'Completed' : 'Failed';
    case 'content-type': return row.contentType ?? '—';
    case 'started-at': return row.startedAt ? new Date(row.startedAt).toLocaleTimeString() : '—';
  }
}
