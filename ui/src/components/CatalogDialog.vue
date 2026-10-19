<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import { X } from 'lucide-vue-next';
import type { DriverCatalog, WorkspacePaths } from '@/types/protocol';

type WorkspaceTarget = { key: string; revision: string; name: string };
export type DialogRequest =
  | { kind: 'new-workspace' }
  | { kind: 'new-session'; workspace: WorkspaceTarget & { invalid: boolean } }
  | { kind: 'rename-workspace'; workspace: WorkspaceTarget }
  | { kind: 'delete-workspace'; workspace: WorkspaceTarget & { lifecycle: string } }
  | { kind: 'rename-agent' | 'delete-agent'; workspace: WorkspaceTarget; agent: { id: string; name: string } };
export interface DialogValue {
  kind: DialogRequest['kind']; key: string; revision: string; agent: string; name: string; projectRoot: string; driver: string;
}
const props = defineProps<{
  request: DialogRequest; drivers: string[]; driverErrors: DriverCatalog['errors']; busy: boolean; offline: boolean;
  queryPaths: (message?: { path?: string; cursor?: string }) => Promise<WorkspacePaths>;
  refreshDrivers: () => Promise<DriverCatalog>;
}>();
const emit = defineEmits<{ close: []; submit: [value: DialogValue] }>();
const name = ref('');
const projectRoot = ref('');
const driver = ref('');
const error = ref('');
const firstField = ref<HTMLInputElement | null>(null);
const dialog = ref<HTMLElement | null>(null);
const returnFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
onBeforeUnmount(() => { if (returnFocus?.isConnected) returnFocus.focus(); });
const browsing = ref(false);
const folderPage = ref<WorkspacePaths | null>(null);
const folderDraft = ref('');
const folderError = ref('');
const loadingFolders = ref(false);
const refreshingDrivers = ref(false);
let generation = 0;
const isDelete = computed(() => props.request.kind.startsWith('delete'));
const titles: Record<DialogRequest['kind'], string> = {
  'new-workspace': 'New workspace', 'new-session': 'New session', 'rename-workspace': 'Rename workspace',
  'delete-workspace': 'Delete workspace?', 'rename-agent': 'Rename agent', 'delete-agent': 'Delete agent?',
};
const title = computed(() => props.request.kind === 'new-session' ? `New session in ${props.request.workspace.name}` : titles[props.request.kind]);
const locked = computed(() => props.busy || loadingFolders.value || refreshingDrivers.value);
const canSubmit = computed(() => {
  if (locked.value || props.offline || browsing.value) return false;
  if (isDelete.value) return true;
  if (!name.value.trim()) return false;
  if (props.request.kind === 'new-workspace') return projectRoot.value.trim().startsWith('/');
  return props.request.kind !== 'new-session' || props.drivers.includes(driver.value);
});
watch(() => props.request, () => {
  generation++;
  name.value = 'agent' in props.request ? props.request.agent.name : props.request.kind === 'rename-workspace' ? props.request.workspace.name : '';
  projectRoot.value = ''; driver.value = ''; error.value = ''; browsing.value = false; folderPage.value = null;
  void nextTick(() => (firstField.value ?? dialog.value?.querySelector<HTMLElement>('button:not(:disabled)'))?.focus());
}, { immediate: true });
watch(() => props.drivers, () => { if (!props.drivers.includes(driver.value)) driver.value = ''; });
function showError(message: string) { error.value = message; }
defineExpose({ showError });
function close() { if (!locked.value) emit('close'); }
async function browse(path?: string, more = false) {
  if (locked.value || props.offline) return;
  if (path && !path.startsWith('/')) { folderError.value = 'Enter an absolute path starting with /'; return; }
  browsing.value = true; loadingFolders.value = true; folderError.value = '';
  const version = generation;
  try {
    const page = await props.queryPaths({ ...(path ? { path } : {}), ...(more && folderPage.value?.next_cursor ? { cursor: folderPage.value.next_cursor } : {}) });
    if (generation !== version) return;
    folderPage.value = more && folderPage.value?.path === page.path
      ? { ...page, directories: [...folderPage.value.directories, ...page.directories].filter((directory, index, rows) => rows.findIndex((row) => row.path === directory.path) === index) } : page;
    folderDraft.value = page.path;
  } catch (failure) { if (generation === version) folderError.value = failure instanceof Error ? failure.message : String(failure); }
  finally { if (generation === version) loadingFolders.value = false; }
}
async function reloadDrivers() {
  if (locked.value || props.offline) return;
  refreshingDrivers.value = true; error.value = '';
  const version = generation;
  try { await props.refreshDrivers(); }
  catch (failure) { if (generation === version) showError(failure instanceof Error ? failure.message : String(failure)); }
  finally { if (generation === version) refreshingDrivers.value = false; }
}
function submit() {
  if (!canSubmit.value) return;
  const request = props.request;
  emit('submit', { kind: request.kind, key: 'workspace' in request ? request.workspace.key : '',
    revision: 'workspace' in request ? request.workspace.revision : '', agent: 'agent' in request ? request.agent.id : '',
    name: name.value.trim(), projectRoot: projectRoot.value.trim(), driver: driver.value });
}
function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') { event.stopPropagation(); if (browsing.value && !locked.value) browsing.value = false; else close(); }
  if (event.key !== 'Tab') return;
  const fields = [...(dialog.value?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex="0"]') ?? [])];
  const first = fields[0]; const last = fields.at(-1);
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
</script>

<template>
  <div class="dialog-backdrop" @click.self="close" @keydown="onKeydown">
    <form ref="dialog" class="dialog catalog-dialog" role="dialog" aria-modal="true" aria-labelledby="catalog-dialog-title" @submit.prevent="submit">
      <header class="dialog-header">
        <h2 id="catalog-dialog-title">{{ title }}</h2>
        <button class="icon-button" type="button" aria-label="Close dialog" :disabled="locked" @click="close"><X :size="17" /></button>
      </header>
      <template v-if="!isDelete">
        <label class="field-label" for="catalog-name">{{ request.kind.startsWith('rename') ? 'New name' : 'Name' }}</label>
        <input id="catalog-name" ref="firstField" v-model="name" class="field-input" :disabled="locked" autocomplete="off" spellcheck="false" />
      </template>
      <template v-if="request.kind === 'new-workspace'">
        <label class="field-label" for="catalog-project-root">Project path on daemon host</label>
        <div class="path-input-row">
          <input id="catalog-project-root" v-model="projectRoot" class="field-input" :disabled="locked" placeholder="/home/me/projects/my-project" autocomplete="off" spellcheck="false" />
          <button class="secondary-button" type="button" :disabled="locked || offline" @click="browse()">Browse</button>
        </div>
        <p class="field-hint">Choose an existing folder on the daemon host, or enter its absolute path.</p>
        <section v-if="browsing" class="folder-browser" aria-label="Choose project folder" :aria-busy="loadingFolders">
          <label class="field-label" for="folder-path">Go to absolute path</label>
          <div class="path-input-row">
            <input id="folder-path" v-model="folderDraft" class="field-input" :disabled="locked" @keydown.enter.prevent="browse(folderDraft.trim())" />
            <button class="secondary-button" type="button" :disabled="locked || !folderDraft.startsWith('/')" @click="browse(folderDraft.trim())">Go</button>
          </div>
          <div class="folder-toolbar">
            <button class="secondary-button" type="button" :disabled="locked || !folderPage?.parent" @click="browse(folderPage?.parent ?? undefined)">Up</button>
            <span>{{ folderPage?.path || 'Daemon home' }}</span>
          </div>
          <p v-if="loadingFolders" role="status">Loading folders…</p>
          <p v-if="folderError" class="form-error" role="alert">{{ folderError }}</p>
          <ul v-if="folderPage" class="folder-list">
            <li v-for="folder in folderPage.directories" :key="folder.path"><button type="button" :disabled="locked" :title="folder.path" @click="browse(folder.path)">{{ folder.name }}/</button></li>
          </ul>
          <p v-if="folderPage && !folderPage.directories.length && !loadingFolders" class="field-hint">No subfolders</p>
          <div class="folder-toolbar">
            <button v-if="folderPage?.next_cursor" class="secondary-button" type="button" :disabled="locked" @click="browse(folderPage.path, true)">Load more</button>
            <button class="secondary-button" type="button" :disabled="locked" @click="browsing = false">Cancel browsing</button>
            <button class="primary-button" type="button" :disabled="locked || !folderPage || !!folderError" @click="projectRoot = folderPage!.path; browsing = false">Select current folder</button>
          </div>
        </section>
      </template>
      <template v-else-if="request.kind === 'new-session'">
        <label class="field-label" for="catalog-agent-driver">Installed driver image</label>
        <select id="catalog-agent-driver" v-model="driver" class="field-input" :disabled="locked || !drivers.length">
          <option disabled value="">Select an installed driver</option>
          <option v-for="option in drivers" :key="option" :value="option">{{ option }}</option>
        </select>
        <p v-if="!drivers.length" class="field-hint" role="status">No validated driver images are available. Install a driver on the daemon host, then refresh.</p>
        <button class="secondary-button" type="button" :disabled="locked || offline" @click="reloadDrivers">{{ refreshingDrivers ? 'Refreshing…' : 'Refresh images' }}</button>
        <p v-for="failure in driverErrors" :key="`${failure.path}:${failure.message}`" class="form-error">{{ failure.path }}: {{ failure.message }}</p>
        <p class="field-hint">Saves an agent definition. If this workspace is running, the daemon also attempts to start it.</p>
      </template>
      <p v-else-if="request.kind === 'rename-workspace'" class="field-hint">Stop the workspace before renaming. Existing session history is preserved.</p>
      <p v-else-if="request.kind === 'rename-agent'" class="field-hint">The daemon stops a running agent, preserves its history under the new name, and attempts to restart it when its workspace is running.</p>
      <p v-else-if="request.kind === 'delete-workspace'" class="dialog-copy">Delete <strong>{{ request.workspace.name }}</strong>? Stop it before deleting. Its session databases are moved intact into the daemon’s internal trash.</p>
      <p v-else-if="request.kind === 'delete-agent'" class="dialog-copy">Remove <strong>{{ request.agent.name }}</strong> from <strong>{{ request.workspace.name }}</strong>? The daemon stops the agent if needed. Its session database is retained.</p>
      <p v-if="offline" class="form-error" role="alert">Connect to the daemon before changing definitions.</p>
      <p v-if="error" class="form-error" role="alert">{{ error }}</p>
      <div class="dialog-actions">
        <button class="secondary-button" type="button" :disabled="locked" @click="close">Cancel</button>
        <button :class="['primary-button', { 'primary-button--danger': isDelete }]" type="submit" :disabled="!canSubmit">{{ busy ? 'Working…' : isDelete ? 'Delete' : request.kind.startsWith('rename') ? 'Save name' : request.kind === 'new-session' ? 'Create agent' : 'Create workspace' }}</button>
      </div>
    </form>
  </div>
</template>
