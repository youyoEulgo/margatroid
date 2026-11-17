<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { storeToRefs } from 'pinia';
import { Plus, Trash2 } from 'lucide-vue-next';
import FormSelect from '@/components/FormSelect.vue';
import { useWorkbenchStore } from '@/stores/workbench';
import {
  MODEL_PROVIDERS,
  parseModelConfig,
  serializeModelConfig,
  supportedApiTypes,
  validateModelRoute,
  type ModelRoute,
} from '@/lib/modelConfig';

const store = useWorkbenchStore();
const { modelConfigContent, modelConfigError, modelConfigLoading } = storeToRefs(store);
const routes = ref<ModelRoute[]>([]);
const initialized = ref(false);
const dirty = ref(false);
const saving = ref(false);
const notice = ref('');
const externalChange = ref(false);
const rowError = ref<number | null>(null);

function blankRoute(): ModelRoute {
  return {
    id: '',
    model: '',
    provider: 'openai',
    apiType: 'chat_completions',
    baseUrl: '',
    apiKey: '',
    contextWindow: '1m',
    thinking: '',
    reasoningEffort: '',
  };
}

const canSave = computed(
  () => initialized.value && dirty.value && !saving.value && !modelConfigLoading.value,
);

watch(
  modelConfigContent,
  (content) => {
    if (content === null) return;
    const parsed = parseModelConfig(content);
    if (!initialized.value || !dirty.value) {
      routes.value = parsed;
      initialized.value = true;
    } else if (serializeModelConfig(parsed) !== serializeModelConfig(routes.value)) {
      externalChange.value = true;
    }
  },
  { immediate: true },
);

function markDirty() {
  dirty.value = true;
  notice.value = '';
  rowError.value = null;
}

function addRoute() {
  routes.value.push(blankRoute());
  markDirty();
}

function removeRoute(index: number) {
  routes.value.splice(index, 1);
  if (rowError.value === index) rowError.value = null;
  else if (rowError.value !== null && rowError.value > index) rowError.value -= 1;
  markDirty();
}

function advancedSupported(route: ModelRoute) {
  return (
    (route.provider === 'deepseek' || route.provider === 'openrouter') &&
    route.apiType === 'chat_completions'
  );
}

function clearUnsupportedAdvanced(route: ModelRoute) {
  if (!advancedSupported(route)) {
    route.thinking = '';
    route.reasoningEffort = '';
  }
}

function providerChanged(route: ModelRoute) {
  const supported = supportedApiTypes(route.provider);
  if (!supported.includes(route.apiType)) route.apiType = supported[0] ?? 'chat_completions';
  clearUnsupportedAdvanced(route);
  markDirty();
}

function apiTypeChanged(route: ModelRoute) {
  clearUnsupportedAdvanced(route);
  markDirty();
}

function thinkingChanged(route: ModelRoute) {
  if (route.thinking !== 'enabled') route.reasoningEffort = '';
  else if (!route.reasoningEffort) route.reasoningEffort = 'high';
  markDirty();
}

function reload() {
  externalChange.value = false;
  notice.value = '';
  dirty.value = false;
  rowError.value = null;
  routes.value = parseModelConfig(modelConfigContent.value);
  initialized.value = modelConfigContent.value !== null;
  store.requestModelConfig();
}

function useServerVersion() {
  if (modelConfigContent.value !== null) routes.value = parseModelConfig(modelConfigContent.value);
  dirty.value = false;
  externalChange.value = false;
  notice.value = '';
}

async function save() {
  if (!canSave.value) return;
  rowError.value = null;
  if (!routes.value.length) {
    notice.value = 'At least one model route is required.';
    return;
  }
  for (const [index, route] of routes.value.entries()) {
    const error = validateModelRoute(route);
    if (error) {
      rowError.value = index;
      notice.value = error;
      return;
    }
  }
  saving.value = true;
  notice.value = '';
  try {
    await store.updateModelConfig(serializeModelConfig(routes.value));
    dirty.value = false;
    externalChange.value = false;
    notice.value = 'Model configuration saved.';
  } catch {
    notice.value = 'Model configuration was not saved — see Activity logs';
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <section class="model-config-panel">
    <div class="model-config-heading">
      <div>
        <strong>models.toml</strong>
        <span>{{ routes.length }} route{{ routes.length === 1 ? '' : 's' }}</span>
      </div>
      <button type="button" class="secondary-button" :disabled="modelConfigLoading" @click="reload">
        Reload
      </button>
    </div>
    <p class="field-hint">
      Global daemon model routes. Every route needs a unique id, a provider/API pair, and valid
      credentials. The composer lists route ids.
    </p>
    <p v-if="externalChange" class="model-config-notice" role="status">
      The model file changed outside this editor.
      <button type="button" class="text-button" @click="useServerVersion">
        Use server version
      </button>
    </p>
    <p v-if="modelConfigError && !initialized" class="form-error">
      Model configuration is unavailable — see Activity logs
    </p>
    <div v-if="routes.length" class="model-route-list">
      <article
        v-for="(route, index) in routes"
        :key="index"
        :class="['model-route-card', { 'model-route-card--invalid': rowError === index }]"
      >
        <div class="model-route-heading">
          <strong>Route {{ index + 1 }}</strong>
          <button
            type="button"
            class="model-route-remove"
            title="Remove route"
            aria-label="Remove route"
            @click="removeRoute(index)"
          >
            <Trash2 :size="14" />
          </button>
        </div>
        <div class="model-route-grid">
          <label>
            <span>Route id</span>
            <input v-model="route.id" class="field-input" @input="markDirty" />
          </label>
          <label>
            <span>Model name</span>
            <input v-model="route.model" class="field-input" @input="markDirty" />
          </label>
          <label>
            <span>Provider</span>
            <FormSelect
              v-model="route.provider"
              :options="MODEL_PROVIDERS"
              @change="providerChanged(route)"
            />
          </label>
          <label>
            <span>API type</span>
            <FormSelect
              v-model="route.apiType"
              :options="supportedApiTypes(route.provider)"
              @change="apiTypeChanged(route)"
            />
          </label>
          <label class="model-route-wide">
            <span>Base URL</span>
            <input
              v-model="route.baseUrl"
              class="field-input"
              placeholder="https://api.example.com/v1"
              @input="markDirty"
            />
          </label>
          <label class="model-route-wide">
            <span>API key</span>
            <input
              v-model="route.apiKey"
              class="field-input"
              type="password"
              autocomplete="off"
              @input="markDirty"
            />
          </label>
          <label>
            <span>Context window</span>
            <input
              v-model="route.contextWindow"
              class="field-input"
              placeholder="1m"
              @input="markDirty"
            />
          </label>
          <label>
            <span>Thinking</span>
            <FormSelect
              v-model="route.thinking"
              :options="['', 'enabled', 'disabled']"
              placeholder="default"
              :disabled="!advancedSupported(route)"
              @change="thinkingChanged(route)"
            />
          </label>
          <label>
            <span>Reasoning effort</span>
            <FormSelect
              v-model="route.reasoningEffort"
              :options="['', 'high', 'max']"
              placeholder="default"
              :disabled="!advancedSupported(route) || route.thinking !== 'enabled'"
              @change="markDirty"
            />
          </label>
        </div>
      </article>
    </div>
    <p v-else class="info-empty">No model routes configured.</p>
    <div class="model-config-actions">
      <button type="button" class="secondary-button" @click="addRoute">
        <Plus :size="14" /> Add route
      </button>
      <span v-if="notice" class="model-config-notice" role="status">{{ notice }}</span>
      <button type="button" class="primary-button" :disabled="!canSave" @click="save">
        {{ saving ? 'Saving…' : 'Save models' }}
      </button>
    </div>
  </section>
</template>
