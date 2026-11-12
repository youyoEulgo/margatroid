import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync } from 'node:fs';
import { mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const output = join(root, 'dist');
const target = resolve(root, '../resources/agent-drivers/local/coder/latest/ui/index.html');
const dependencies = join(root, 'node_modules');

// The Driver UI shares the platform protocol, stores and dependencies. Link the
// existing install instead of duplicating Vue/Pinia and diverging at runtime.
if (!existsSync(dependencies)) symlink('../ui/node_modules', dependencies, 'dir');
if (!lstatSync(dependencies).isSymbolicLink() && !existsSync(join(dependencies, 'vite')))
  throw new Error('driver_ui/node_modules must contain the ui dependencies');

const build = spawnSync('bun', ['x', 'vite', 'build'], { cwd: root, stdio: 'inherit' });
if (build.status !== 0) process.exit(build.status ?? 1);

let html = await readFile(join(output, 'index.html'), 'utf8');
const inline = async (src) => {
  const path = join(output, src.replace(/^\//, ''));
  const content = await readFile(path, 'utf8');
  return path.endsWith('.css')
    ? `<style>${content}</style>`
    : `<script type="module">${content.replaceAll('</script', '<\\/script')}</script>`;
};

for (const match of html.matchAll(/<link[^>]+rel="modulepreload"[^>]*>/g))
  html = html.replace(match[0], '');
for (const match of html.matchAll(/<script[^>]*src="([^"]+)"[^>]*><\/script>/g)) {
  const value = await inline(match[1]);
  html = html.replace(match[0], () => value);
}
for (const match of html.matchAll(/<link[^>]+rel="stylesheet"[^>]*href="([^"]+)"[^>]*>/g)) {
  const value = await inline(match[1]);
  html = html.replace(match[0], () => value);
}

if (/<script[^>]*src=|<link[^>]+rel="stylesheet"/.test(html))
  throw new Error('Driver UI still references external assets');
await mkdir(dirname(target), { recursive: true });
await writeFile(target, html);
await rm(output, { recursive: true, force: true });
console.log(`driver ui written: ${target} (${Buffer.byteLength(html)} bytes)`);
