// Tool calls carry JSON (arguments and often results). JSON is correct on the
// wire but noisy to read: escaped quotes, \n, and one long line. Render the
// same value as a compact TOML-like document instead.

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function parsePayload(value: unknown): unknown {
  if (typeof value !== 'string') return value;
  const text = value.trim();
  if (!text || !/^[[{"]/.test(text)) return value;
  try {
    return JSON.parse(text);
  } catch {
    return value;
  }
}

const BARE_KEY = /^[A-Za-z0-9_-]+$/;

function tomlString(value: string): string {
  // Literal strings keep backslashes and embedded double quotes readable.
  if (value.includes('\n')) {
    if (!value.includes("'''")) return `'''${value}'''`;
    return JSON.stringify(value);
  }
  if (!value.includes("'")) return `'${value}'`;
  return JSON.stringify(value);
}

function tomlKey(key: string): string {
  return BARE_KEY.test(key) ? key : tomlString(key);
}

function tomlValue(value: unknown, indent = ''): string {
  if (value === null) return 'null';
  if (typeof value === 'string') return tomlString(value);
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value)) {
    if (!value.length) return '[]';
    const items = value.map((item) => tomlValue(item, `${indent}  `));
    return `[\n${items.map((item) => `${indent}  ${item},`).join('\n')}\n${indent}]`;
  }
  if (isRecord(value)) {
    const entries = Object.entries(value);
    if (!entries.length) return '{}';
    return `{ ${entries.map(([key, item]) => `${tomlKey(key)} = ${tomlValue(item, indent)}`).join(', ')} }`;
  }
  return String(value);
}

function emitTable(lines: string[], path: string[], table: Record<string, unknown>): void {
  const scalars = Object.entries(table).filter(
    ([, value]) => !(isRecord(value) && Object.keys(value).length),
  );
  for (const [key, value] of scalars) lines.push(`${tomlKey(key)} = ${tomlValue(value)}`);
  for (const [key, value] of Object.entries(table)) {
    if (!isRecord(value) || !Object.keys(value).length) continue;
    const nextPath = [...path, key];
    lines.push('', `[${nextPath.map(tomlKey).join('.')}]`);
    emitTable(lines, nextPath, value);
  }
}

/**
 * Render a tool argument/result as a TOML-like document. Plain text (shell
 * output, error strings) is returned untouched; JSON strings are parsed so
 * escaped quotes and newlines disappear.
 */
export function formatToolPayload(value: unknown): string {
  const parsed = parsePayload(value);
  if (isRecord(parsed)) {
    if (!Object.keys(parsed).length) return '{}';
    const lines: string[] = [];
    emitTable(lines, [], parsed);
    return lines.join('\n').trim();
  }
  if (Array.isArray(parsed)) return tomlValue(parsed);
  if (typeof parsed === 'string') return parsed;
  return String(parsed ?? '');
}

/** One-line variant for the collapsed tool summary. */
export function inlineToolPayload(value: unknown): string {
  return formatToolPayload(value).replace(/\n\s*/g, ' ').trim();
}
