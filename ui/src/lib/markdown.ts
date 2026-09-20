import DOMPurify from 'dompurify';
import katex from 'katex';
import { Marked, type Tokens } from 'marked';

type MathToken = Tokens.Generic & {
  text: string;
  displayMode: boolean;
};

const blockMath =
  /^(?:\$\$[ \t]*\n?([\s\S]+?)\n?[ \t]*\$\$|\\\[[ \t]*\n?([\s\S]+?)\n?[ \t]*\\\])(?:\n|$)/;
const inlineDisplayMath =
  /^(?:\$\$[ \t]*\n?([\s\S]+?)\n?[ \t]*\$\$|\\\[[ \t]*\n?([\s\S]+?)\n?[ \t]*\\\])/;
const inlineMath = /^(?:\\\((.+?)\\\)|\$(?!\$|\s)((?:\\.|[^\\$\n])+?)(?<!\s)\$(?!\$))/;

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

export function renderMarkdown(content: string): string {
  const rendered = markdown.parse(content) as string;
  // Marked keeps the trailing newline as a text node after the last block,
  // which creates a second, empty line inside the inline-block user bubble.
  return DOMPurify.sanitize(rendered).trim();
}
