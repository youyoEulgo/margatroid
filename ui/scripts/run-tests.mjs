import { spawnSync } from 'node:child_process';
import { mkdtemp, realpath, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const temporaryRoot = await realpath(tmpdir());
const outputDirectory = await mkdtemp(join(temporaryRoot, 'margatroid-ui-tests-'));
const resolvedOutput = await realpath(outputDirectory);
if (dirname(resolvedOutput) !== temporaryRoot || !basename(resolvedOutput).startsWith('margatroid-ui-tests-')) {
  throw new Error('Test output directory is not the expected temporary child');
}
async function cleanTestOutput() {
  const cleanupTarget = await realpath(outputDirectory);
  if (cleanupTarget !== resolvedOutput || dirname(cleanupTarget) !== temporaryRoot) {
    throw new Error('Refusing to clean up an unexpected test output directory');
  }
  await rm(cleanupTarget, { recursive: true });
}

try {
  await build({
    root,
    build: {
      lib: { entry: join(root, 'src/stores/workbench.test.ts'), formats: ['es'] },
      minify: false,
      outDir: resolvedOutput,
      emptyOutDir: false,
      rollupOptions: {
        external: ['node:test', 'node:assert'],
        output: { entryFileNames: 'workbench.test.mjs' },
      },
    },
    ssr: { noExternal: true },
  });
  const result = spawnSync(process.execPath, ['--test', join(resolvedOutput, 'workbench.test.mjs')], {
    cwd: root,
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  process.exitCode = result.status ?? 1;
} finally {
  await cleanTestOutput();
}
