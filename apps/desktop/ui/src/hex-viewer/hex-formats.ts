export type HexCopyFormat = 'hex' | 'literal' | 'utf8' | 'c' | 'go' | 'java' | 'json' | 'base64';

export function encodeBytes(bytes: Uint8Array): string {
  const chunks: string[] = [];
  for (let start = 0; start < bytes.length; start += 8192) {
    chunks.push(String.fromCharCode(...bytes.subarray(start, start + 8192)));
  }
  return btoa(chunks.join(''));
}

export function decodeBytes(encoded: string): Uint8Array {
  const binary = atob(encoded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export function utf8Selection(bytes: Uint8Array): string | null {
  try {
    const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes);
    const nul = text.indexOf('\0');
    return nul < 0 ? text : text.slice(0, nul);
  } catch { return null; }
}

export function formatHexSelection(bytes: Uint8Array, format: HexCopyFormat): string | null {
  if (format === 'base64') return encodeBytes(bytes);
  if (format === 'utf8') return utf8Selection(bytes);
  if (format === 'json') return JSON.stringify(bytes);
  const hex = Array.from(bytes, byte => byte.toString(16).padStart(2, '0'));
  if (format === 'hex') return hex.join('');
  if (format === 'literal') return hex.map(byte => '\\x' + byte).join('');
  const lines: string[] = [];
  for (let start = 0; start < hex.length; start += 8) {
    lines.push('\t' + hex.slice(start, start + 8).map(byte => '0x' + byte + ', ').join(''));
  }
  const body = lines.join('\n');
  if (format === 'go') return 'var from_transmog = []byte{\n' + body + '\n}';
  const declaration = format === 'c'
    ? 'unsigned char from_transmog[' + bytes.length + '] ='
    : 'byte from_transmog[] =';
  return declaration + '\n{\n' + body + '\n};';
}
