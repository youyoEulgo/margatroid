import type { AgentUiBundle, AgentUiFile } from '@/types/protocol';

export interface DriverBootstrap {
  endpoint: string;
  address: string;
  contextPath?: string;
}

function decodeBase64(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

function textFile(file: AgentUiFile): string {
  return new TextDecoder().decode(decodeBase64(file.data));
}

function dataUrl(file: AgentUiFile): string {
  return `data:${file.media_type};base64,${file.data}`;
}

function normalizePath(path: string): string {
  return path.replace(/^\.?\//, '');
}

function escapeScript(value: string): string {
  return value.replaceAll('</script', '<\\/script');
}

/**
 * The platform receives Driver UI as a set of files. srcdoc has no directory
 * to resolve relative URLs against, so referenced entry assets are inlined and
 * binary resources become data URLs. A single-file Driver build passes through.
 */
export function buildDriverDocument(bundle: AgentUiBundle, bootstrap: DriverBootstrap): string {
  const files = new Map(bundle.files.map((file) => [normalizePath(file.path), file]));
  const entry = files.get(normalizePath(bundle.entry));
  if (!entry) throw new Error(`Driver UI entry ${bundle.entry} is missing`);
  let html = textFile(entry);

  html = html.replace(/<link[^>]+rel="modulepreload"[^>]*>/gi, '');
  html = html.replace(
    /<script([^>]*)\ssrc="([^"]+)"([^>]*)><\/script>/gi,
    (match, before: string, source: string, after: string) => {
      const file = files.get(normalizePath(source));
      if (!file) return match;
      return `<script${before}${after}>${escapeScript(textFile(file))}</script>`;
    },
  );
  html = html.replace(
    /<link([^>]+)rel="stylesheet"([^>]*)href="([^"]+)"([^>]*)>/gi,
    (match, before: string, middle: string, source: string) => {
      const file = files.get(normalizePath(source));
      if (!file) return match;
      return `<style>${textFile(file)}</style>`;
    },
  );
  html = html.replace(
    /(<(?:img|source|video|audio|link)[^>]*\s(?:src|href)=")([^"]+)(")/gi,
    (match, before: string, source: string, after: string) => {
      if (source.startsWith('data:') || source.startsWith('http:') || source.startsWith('https:'))
        return match;
      const file = files.get(normalizePath(source));
      return file ? `${before}${dataUrl(file)}${after}` : match;
    },
  );

  const boot = `<script>window.__MARGATROID_DRIVER__=${JSON.stringify(bootstrap)
    .replaceAll('<', '\\u003c')
    .replaceAll('>', '\\u003e')};</script>`;
  const head = html.lastIndexOf('</head>');
  return head >= 0 ? `${html.slice(0, head)}${boot}${html.slice(head)}` : `${boot}${html}`;
}
