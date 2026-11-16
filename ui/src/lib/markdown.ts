import DOMPurify from 'dompurify';
import katex from 'katex';
import { Marked, type Tokens } from 'marked';
import { workspaceFileUrl, type MarkdownResourceContext } from '@/lib/workspaceUrl';

type MathToken = Tokens.Generic & {
  text: string;
  displayMode: boolean;
};

const blockMath =
  /^(?:\$\$[ \t]*\n?([\s\S]+?)\n?[ \t]*\$\$|\\\[[ \t]*\n?([\s\S]+?)\n?[ \t]*\\\])(?:\n|$)/;
const inlineDisplayMath =
  /^(?:\$\$[ \t]*\n?([\s\S]+?)\n?[ \t]*\$\$|\\\[[ \t]*\n?([\s\S]+?)\n?[ \t]*\\\])/;
const inlineMath = /^(?:\\\((.+?)\\\)|\$(?!\$|\s)((?:\\.|[^\\$\n])+?)(?<!\s)\$(?!\$))/;

// Marked's inlineText consumes runs like "$x$" before a custom inline
// tokenizer is consulted. Tell the lexer where the next math delimiter is.
const mathStart = (source: string): number | undefined => {
  const index = source.search(/\$|\\\(|\\\[/);
  return index < 0 ? undefined : index;
};

function renderMath(token: MathToken): string {
  const formula = katex.renderToString(token.text, {
    displayMode: token.displayMode,
    throwOnError: false,
    strict: 'warn',
    trust: false,
  });

  return token.displayMode
    ? `<span class="math-display">${formula}</span>\n`
    : `<span class="math-inline">${formula}</span>`;
}

const markdown = new Marked({
  gfm: true,
  breaks: true,
});

markdown.use({
  extensions: [
    {
      name: 'blockMath',
      level: 'block',
      tokenizer(source) {
        const match = blockMath.exec(source);
        if (!match) return;

        return {
          type: 'blockMath',
          raw: match[0],
          text: (match[1] ?? match[2] ?? '').trim(),
          displayMode: true,
        };
      },
      renderer(token) {
        return renderMath(token as MathToken);
      },
    },
    {
      name: 'inlineDisplayMath',
      level: 'inline',
      start: mathStart,
      tokenizer(source) {
        const match = inlineDisplayMath.exec(source);
        if (!match) return;

        return {
          type: 'inlineDisplayMath',
          raw: match[0],
          text: (match[1] ?? match[2] ?? '').trim(),
          displayMode: true,
        };
      },
      renderer(token) {
        return renderMath(token as MathToken);
      },
    },
    {
      name: 'inlineMath',
      level: 'inline',
      start: mathStart,
      tokenizer(source) {
        const match = inlineMath.exec(source);
        if (!match) return;

        return {
          type: 'inlineMath',
          raw: match[0],
          text: match[1] ?? match[2] ?? '',
          displayMode: false,
        };
      },
      renderer(token) {
        return renderMath(token as MathToken);
      },
    },
  ],
});

type ResourceRewriter = (value: string) => string | undefined;
let activeRewriter: ResourceRewriter | null = null;

DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  if (!activeRewriter) return;
  const element = node as Element;
  if (element.tagName === 'IMG') {
    const src = element.getAttribute('src');
    if (src) {
      const rewritten = activeRewriter(src);
      if (rewritten) element.setAttribute('src', rewritten);
    }
  } else if (element.tagName === 'A') {
    const href = element.getAttribute('href');
    if (href) {
      const rewritten = activeRewriter(href);
      if (rewritten) element.setAttribute('href', rewritten);
    }
  }
});

const cache = new Map<string, string>();
const CACHE_LIMIT = 400;

export function renderMarkdown(content: string, context: MarkdownResourceContext = {}): string {
  const key = `${context.workspace ?? ''}\n${context.endpoint ?? ''}\n${context.baseDir ?? ''}\n${content}`;
  const cached = cache.get(key);
  if (cached !== undefined) return cached;
  const rendered = markdown.parse(content) as string;
  activeRewriter = (value) => workspaceFileUrl(value, context);
  let html: string;
  try {
    // Marked keeps the trailing newline as a text node after the last block,
    // which creates a second, empty line inside the inline-block user bubble.
    html = DOMPurify.sanitize(rendered).trim();
  } finally {
    activeRewriter = null;
  }
  // Follow-up renders of unchanged messages are common while another message
  // is streaming, so keep recent results and bound the map.
  cache.set(key, html);
  if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
  return html;
}
