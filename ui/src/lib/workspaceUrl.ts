export interface MarkdownResourceContext {
  /** Full Workspace Address, e.g. workspace:local/demo:latest. */
  workspace?: string;
  /** Current daemon WebSocket endpoint; ws/wss determines http/https. */
  endpoint?: string;
  /** Directory containing the Markdown source, relative to the workspace root. */
  baseDir?: string;
}

/**
 * Resolve a Markdown resource URL against a Workspace's HTTP file route:
 * /{encodeURIComponent(workspace.address)}/{relative path}.
 */
export function workspaceFileUrl(
  value: string,
  context: MarkdownResourceContext,
): string | undefined {
  if (!value || value.startsWith('#') || value.startsWith('/') || value.startsWith('//'))
    return undefined;
  if (/^[a-z][a-z0-9+.-]*:/i.test(value)) return undefined;
  if (!context.workspace || !context.endpoint) return undefined;
  if (value.includes('\\')) return undefined;

  const [pathPart, suffix = ''] = value.split(/(?=[?#])/u);
  if (!pathPart) return undefined;
  const parts = [...(context.baseDir ?? '').split('/').filter(Boolean), ...pathPart.split('/')];
  const normalized: string[] = [];
  for (const part of parts) {
    if (!part || part === '.') continue;
    if (part === '..') {
      if (!normalized.length) return undefined;
      normalized.pop();
      continue;
    }
    normalized.push(encodeURIComponent(part));
  }
  if (!normalized.length) return undefined;

  let url: URL;
  try {
    url = new URL(context.endpoint);
  } catch {
    return undefined;
  }
  const origin = `${url.protocol === 'wss:' ? 'https:' : 'http:'}//${url.host}`;
  return `${origin}/${encodeURIComponent(context.workspace)}/${normalized.join('/')}${suffix}`;
}
