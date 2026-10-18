import type { ColumnId, ColumnPreference, SessionSummary, WorkspacePreferences } from './models.js';
import initialState from './initial-state.json';

export const columnDefinitions: ReadonlyArray<{id: ColumnId; label: string; numeric: boolean; placeholder: string}> = [
  {id:'request-number', label:'Request order', numeric:true, placeholder:'Positive request number'},
  {id:'method', label:'Method', numeric:false, placeholder:'GET, POST…'},
  {id:'status', label:'Status', numeric:true, placeholder:'HTTP status, e.g. 304'},
  {id:'process', label:'Process / PID', numeric:false, placeholder:'Process name'},
  {id:'host', label:'Host', numeric:false, placeholder:'example.com'},
  {id:'path', label:'Path and query', numeric:false, placeholder:'/api/'},
  {id:'duration', label:'Duration', numeric:true, placeholder:'Milliseconds'},
  {id:'response-bytes', label:'Response size', numeric:true, placeholder:'Bytes'},
  {id:'protocol', label:'Protocol', numeric:false, placeholder:'HTTP/1.1'},
  {id:'request-bytes', label:'Request size', numeric:true, placeholder:'Bytes'},
  {id:'state', label:'State', numeric:false, placeholder:'active, completed, cancelled, failed'},
  {id:'content-type', label:'Content type', numeric:false, placeholder:'application/json'},
  {id:'started-at', label:'Start time', numeric:true, placeholder:'Unix milliseconds'},
  {id:'pid', label:'Process ID', numeric:true, placeholder:'PID'},
  {id:'url', label:'Full URL', numeric:false, placeholder:'https://example.com/api/'},
  {id:'stream-sent',label:'Stream sent',numeric:true,placeholder:'Decoded message bytes'},
  {id:'stream-received',label:'Stream received',numeric:true,placeholder:'Decoded message bytes'},
  {id:'http-duration', label:'HTTP duration', numeric:true, placeholder:'Milliseconds'},
];

export function defaultWorkspace(): WorkspacePreferences {
  return structuredClone(initialState.workspace) as WorkspacePreferences;
}

export function normalizeWorkspace(value: WorkspacePreferences | undefined): WorkspacePreferences {
  const fallback = defaultWorkspace();
  if (!value || !Array.isArray(value.columns)) return fallback;
  if ([value.listSplit,value.requestSplit,value.requestBodySplit,value.responseBodySplit,value.autoresponseSplit].some(split=>split!==undefined&&(!Number.isInteger(split)||split<15||split>85))) return fallback;
  const known = new Set(columnDefinitions.map((column) => column.id));
  const ids = new Set(value.columns.map((column) => column.id));
  if (ids.size !== value.columns.length || value.columns.some((column) => !known.has(column.id) || Object.keys(column).some(key=>!['id','width','visible','pinned'].includes(key)) || !Number.isInteger(column.width) || column.width<32 || column.width>1200 || typeof column.visible!=='boolean' || typeof column.pinned!=='boolean') || !value.columns.some(column=>column.visible)) return fallback;
  let columns = value.columns.map(column=>({...column}));
  const missing = fallback.columns.filter(column=>!ids.has(column.id));
  if (missing.some(column=>!['request-number','http-duration','stream-sent','stream-received'].includes(column.id))) return fallback;
  if (!ids.has('request-number')) columns.unshift({...fallback.columns[0]!});
  for(const id of ['http-duration','stream-sent','stream-received'])if(!ids.has(id as ColumnId))columns.push({...fallback.columns.find(column=>column.id===id)!});
  return { ...fallback, ...value, wrapCells:false, columns };
}

export function displayColumns(columns: ColumnPreference[]) {
  let offset = 0;
  return [...columns.filter((column) => column.visible && column.pinned), ...columns.filter((column) => column.visible && !column.pinned)].map((column) => {
    const definition = columnDefinitions.find((definition) => definition.id === column.id)!;
    const result = {...column,...definition,headerLabel:column.id==='request-number'?'#':definition.label,widthCss:column.width+'px',offsetCss:offset+'px'};
    if (column.pinned) offset += column.width;
    return result;
  });
}

export function formatBytes(value: number): string {
  if (value < 1024) return value + ' B';
  if (value < 1024*1024) return (value/1024).toFixed(1) + ' KB';
  return (value/(1024*1024)).toFixed(1) + ' MB';
}

type ResponseStatus = Pick<SessionSummary, 'status' | 'terminal'>;

export function statusText(row: ResponseStatus): string {
  if (row.terminal === 'cancelled') return (row.status === null ? '' : row.status+' · ')+'Cancelled by caller';
  if (row.status === null) return row.terminal === 'active' ? 'Pending' : row.terminal === 'failed' ? 'Failed' : 'Unavailable';
  return row.status === 304 ? '304 Not Modified' : String(row.status);
}

export function statusTone(row: ResponseStatus): string {
  if (row.terminal === 'cancelled') return 'cancelled';
  if (row.terminal === 'failed') return 'failed';
  if (row.status === 304) return 'not-modified';
  if (row.status === null) return 'pending';
  if (row.status >= 500) return 'failed';
  if (row.status >= 400) return 'warning';
  if (row.status >= 300) return 'redirect';
  return 'success';
}

export function stateText(state: SessionSummary['terminal']): string {
  return {active:'In progress',completed:'Completed',failed:'Failed',cancelled:'Cancelled by caller',interrupted:'Interrupted','open-at-capture':'Open at capture',incomplete:'Incomplete'}[state];
}

export function formatDuration(value:number|null|undefined):string {
  if(value==null)return 'Unavailable';
  if(value<1000)return value+' ms';
  if(value<60_000)return (value/1000).toFixed(1).replace(/\.0$/,'')+' s';
  const seconds=Math.floor(value/1000),minutes=Math.floor(seconds/60);
  return minutes<60?minutes+'m '+seconds%60+'s':Math.floor(minutes/60)+'h '+minutes%60+'m';
}
export function lifetimeDescription(lifetime:SessionSummary['streamLifetime']):string {
  if(!lifetime)return 'Stream lifetime unavailable.';
  const duration=formatDuration(lifetime.durationMs);
  return lifetime.boundary==='never-established'?'Lifetime unavailable: no established stream was observed.':lifetime.boundary==='live'?'Open for '+duration+'; lifetime continues while quiet.':lifetime.boundary==='final'?'Lifetime '+duration+'; stopped at the recorded stream end.':lifetime.boundary==='open-at-capture'?'Observed lifetime '+duration+'; open at capture. Saved traffic does not age.':'Observed lifetime '+duration+'; end timing is incomplete.';
}
export function cellText(row: SessionSummary, column: ColumnId): string {
  switch (column) {
    case 'request-number': return row.requestNumber ?? '—';
    case 'method': return row.method;
    case 'status': return row.terminal === 'cancelled' ? 'Cancelled' : statusText(row);
    case 'process': return row.caller.kind === 'remote' ? 'Remote' : (row.caller.processName ?? 'Unknown process') + (row.caller.processId === null ? '' : ' ('+row.caller.processId+')');
    case 'pid': return row.caller.processId?.toString() ?? '—';
    case 'host': return row.host;
    case 'path': return row.path;
    case 'url': return row.url ?? row.host+row.path;
    case 'protocol': return row.protocol === 'Http1' ? 'HTTP/1.1' : row.protocol === 'Http2' ? 'HTTP/2' : row.protocol;
    case 'duration': return row.durationMs == null ? '—' : row.streamProtocol ? (row.streamLifetime?.boundary==='open-at-capture'||row.streamLifetime?.boundary==='incomplete'?'≥ ':'')+formatDuration(row.durationMs)+(row.streamLifetime?.live?' · live':'') : row.durationMs+' ms';
    case 'http-duration': return row.httpDurationMs == null ? '—' : row.httpDurationMs+' ms';
    case 'stream-sent': return row.streamSent==null?'—':formatBytes(row.streamSent);
    case 'stream-received': return row.streamReceived==null?'—':formatBytes(row.streamReceived);
    case 'response-bytes': return formatBytes(row.responseBytes);
    case 'request-bytes': return formatBytes(row.requestBytes);
    case 'state': return stateText(row.terminal);
    case 'content-type': return row.contentType ?? '—';
    case 'started-at': return row.startedAt ? new Date(row.startedAt).toLocaleTimeString() : '—';
  }
}
