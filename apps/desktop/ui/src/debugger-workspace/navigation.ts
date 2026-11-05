export interface DebuggerFile {
  id: string;
  group: string;
  package: string;
  path: string;
  artifact: string;
  source: string;
  language: string;
  state: string;
  counterpart: string | null;
}
export interface SourceNode {
  key: string;
  label: string;
  open: boolean;
  selected: boolean;
  file?: DebuggerFile | undefined;
  children: SourceNode[];
}
export interface SourceResult {
  key: string;
  file: DebuggerFile;
  label: string;
  snippet: string;
  line: number;
  column: number;
  length: number;
  selected: boolean;
}
export const sourceKey = (file: DebuggerFile): string => `${file.id}@${file.artifact}`;

export function sourceTree(
  files: DebuggerFile[],
  expanded: Map<string, boolean>,
  selected: string,
): SourceNode[] {
  const roots: SourceNode[] = [];
  const folders = new Map<string, SourceNode>();
  for (const file of files) {
    const path = file.path.replaceAll("\\", "/").split("/");
    const segments = [
      file.group,
      file.package,
      ...path.slice(0, 30),
      ...(path.length > 30 ? [path.slice(30).join("/")] : []),
    ];
    let children = roots;
    let key = "";
    for (let index = 0; index < segments.length - 1; index++) {
      key += `/${segments[index]}`;
      let folder = folders.get(key);
      if (!folder) {
        folder = {
          key,
          label: segments[index]!,
          open: expanded.get(key) ?? file.group === "User scripts",
          selected: false,
          children: [],
        };
        folders.set(key, folder);
        children.push(folder);
      }
      children = folder.children;
    }
    children.push({
      key: sourceKey(file),
      label: segments.at(-1)!,
      file,
      open: false,
      selected: sourceKey(file) === selected,
      children: [],
    });
  }
  const sort = (nodes: SourceNode[]): void => {
    nodes.sort(
      (left, right) =>
        Number(!!left.file) - Number(!!right.file) || left.label.localeCompare(right.label),
    );
    for (const node of nodes) {
      sort(node.children);
    }
  };
  sort(roots);
  return roots;
}

export function matchingFiles(
  files: DebuggerFile[],
  query: string,
  current?: DebuggerFile,
): SourceResult[] {
  const suffix = /^(.*?):(\d+)(?::(\d+))?$/u.exec(query.trim());
  const line = suffix ? Number(suffix[2]) : 0;
  const column = suffix?.[3] ? Number(suffix[3]) : 1;
  if (
    suffix &&
    (!Number.isSafeInteger(line) || line < 1 || !Number.isSafeInteger(column) || column < 1)
  ) {
    return [];
  }
  const needle = (suffix ? suffix[1]! : query).toLocaleLowerCase().trim();
  const candidates = suffix && !needle ? (current ? [current] : []) : files;
  return candidates
    .filter(
      (file) =>
        `${file.package}/${file.path}`.toLocaleLowerCase().includes(needle) ||
        file.source.toLocaleLowerCase().includes(needle),
    )
    .slice(0, 100)
    .map((file, index) => ({
      key: sourceKey(file),
      file,
      label: `${file.package}/${file.path}${line ? `:${line}${suffix?.[3] ? `:${column}` : ""}` : ""}`,
      snippet: file.state,
      line,
      column,
      length: 0,
      selected: index === 0,
    }));
}

/** Bounded catalog search. Reading a verified source never starts its worker. */
export async function searchSources(
  files: DebuggerFile[],
  query: string,
  read: (file: DebuggerFile) => Promise<string>,
  current: () => boolean,
): Promise<{ results: SourceResult[]; limited: boolean }> {
  const needle = query.trim();
  const results: SourceResult[] = [];
  let remaining = 8 * 1024 * 1024;
  let limited = files.length > 256;
  if (!needle) {
    return { results, limited: false };
  }
  const pattern = new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&"), "giu");
  for (const file of files.slice(0, 256)) {
    if (!current()) {
      break;
    }
    let text;
    try {
      text = await read(file);
    } catch {
      limited = true;
      continue;
    }
    if (!current()) {
      break;
    }
    remaining -= text.length;
    if (remaining < 0) {
      limited = true;
      break;
    }
    let offset = 0;
    let line = 1;
    while (offset <= text.length) {
      const end = text.indexOf("\n", offset);
      const value = text.slice(offset, end < 0 ? undefined : end);
      for (const match of value.matchAll(pattern)) {
        if (results.length === 100) {
          return { results, limited: true };
        }
        const start = Math.max(0, match.index - 60);
        results.push({
          key: `${sourceKey(file)}:${line}:${match.index + 1}`,
          file,
          label: `${file.package}/${file.path}:${line}:${match.index + 1}`,
          snippet: (start ? "…" : "") + value.slice(start, start + 240),
          line,
          column: match.index + 1,
          length: match[0].length,
          selected: results.length === 0,
        });
      }
      if (end < 0) {
        break;
      }
      offset = end + 1;
      line++;
    }
  }
  return { results, limited };
}
