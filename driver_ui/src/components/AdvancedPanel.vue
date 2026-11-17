<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { isRecord, isAgentMessage, type AgentMessage } from "@/types/protocol";
import { useWorkbenchStore } from "@/stores/workbench";
import { imageSource } from "@/lib/images";
import { X } from "lucide-vue-next";
import FormSelect from "@/components/FormSelect.vue";

interface SchemaField {
  key: string;
  schema: Record<string, unknown>;
  required: boolean;
}

interface InitField {
  key: string;
  label: string;
  placeholder?: string;
  type?: "text" | "json" | "boolean";
  wide?: boolean;
}
interface InitTemplate {
  id: string;
  label: string;
  fields: InitField[];
}
interface CustomInit {
  template: string;
  fields: Record<string, string | boolean>;
  command: string;
  argumentsText: string;
}

const INIT_TEMPLATES: InitTemplate[] = [
  {
    id: "import_tool",
    label: "IMPORT TOOL",
    fields: [
      {
        key: "resource",
        label: "Tool resource",
        placeholder: "tool:local/read-file:latest",
      },
      { key: "alias", label: "Alias", placeholder: "read_file" },
    ],
  },
  {
    id: "import_message",
    label: "IMPORT MESSAGE",
    fields: [
      {
        key: "resource",
        label: "Message resource",
        placeholder: "prompt:local/coder-soul:latest",
      },
      { key: "alias", label: "Alias", placeholder: "soul" },
    ],
  },
  {
    id: "import_sandbox",
    label: "IMPORT SANDBOX",
    fields: [
      {
        key: "resource",
        label: "Sandbox resource",
        placeholder: "sandbox:local/read-only:latest",
      },
      { key: "alias", label: "Alias", placeholder: "read_only" },
    ],
  },
  {
    id: "inject",
    label: "INJECT messages",
    fields: [
      {
        key: "target",
        label: "Target",
        placeholder: "session.conversation[-1]",
      },
      {
        key: "messages",
        label: "Messages (JSON array)",
        type: "json",
        wide: true,
        placeholder: '[{"type":"system","content":""}]',
      },
    ],
  },
  {
    id: "config_set",
    label: "CONFIG SET",
    fields: [
      {
        key: "path",
        label: "Dotted path",
        placeholder: "compaction.threshold",
      },
      { key: "value", label: "Value (JSON)", type: "json", placeholder: "0.8" },
    ],
  },
  {
    id: "config_delete",
    label: "CONFIG DELETE",
    fields: [
      { key: "path", label: "Dotted path", placeholder: "custom.option" },
    ],
  },
  {
    id: "create_block",
    label: "CREATE BLOCK",
    fields: [
      { key: "name", label: "Block name", placeholder: "session" },
      { key: "fields", label: "Fields", placeholder: "conversation" },
      { key: "temp", label: "Temporary", type: "boolean" },
    ],
  },
  {
    id: "snapshot",
    label: "EFFECT block_snapshot",
    fields: [{ key: "block", label: "Block", placeholder: "session" }],
  },
  {
    id: "restore",
    label: "EFFECT block_restore",
    fields: [
      { key: "block", label: "Block", placeholder: "session" },
      { key: "snapshot", label: "Snapshot id", placeholder: "1" },
    ],
  },
  {
    id: "sandbox_use",
    label: "EFFECT sandbox_use",
    fields: [
      {
        key: "aliases",
        label: "Aliases (JSON array)",
        type: "json",
        wide: true,
        placeholder: '["read_only"]',
      },
    ],
  },
  {
    id: "tool_call",
    label: "EFFECT tool_call",
    fields: [
      {
        key: "calls",
        label: "Tool calls (JSON array)",
        type: "json",
        wide: true,
        placeholder: '[{"id":"call","tool_name":"read_file","arguments":"{}"}]',
      },
    ],
  },
  {
    id: "effect",
    label: "EFFECT (no arguments)",
    fields: [
      {
        key: "effect",
        label: "Effect",
        placeholder: "wake / start_turn / finish_turn / ready",
      },
    ],
  },
  {
    id: "custom",
    label: "Custom command",
    fields: [
      {
        key: "command",
        label: "MCL command",
        wide: true,
        placeholder: "GET session.conversation",
      },
      {
        key: "arguments",
        label: "Arguments (JSON array)",
        type: "json",
        wide: true,
        placeholder: "[]",
      },
    ],
  },
];

function templateById(id: string): InitTemplate {
  return (
    INIT_TEMPLATES.find((template) => template.id === id) ?? INIT_TEMPLATES[0]!
  );
}

function templateFields(id: string): InitField[] {
  return templateById(id).fields;
}

function initFields(id: string): Record<string, string | boolean> {
  const fields: Record<string, string | boolean> = {};
  for (const field of templateFields(id))
    fields[field.key] = field.type === "boolean" ? false : "";
  return fields;
}

function jsonField(value: string): unknown {
  try {
    return JSON.parse(value);
  } catch {
    return undefined;
  }
}

function storedArguments(item: Record<string, unknown>): unknown[] {
  return Array.isArray(item.arguments) ? item.arguments : [];
}

function inferTemplate(
  command: string,
  args: unknown[],
): { template: string; fields: Record<string, string | boolean> } {
  const value = (index: number) => JSON.stringify(args[index] ?? null);
  let match = /^IMPORT (TOOL|MESSAGE|SANDBOX)\s+(\S+)\s+AS\s+(\S+)/u.exec(
    command,
  );
  if (match)
    return {
      template:
        match[1] === "TOOL"
          ? "import_tool"
          : match[1] === "MESSAGE"
            ? "import_message"
            : "import_sandbox",
      fields: { resource: match[2] ?? "", alias: match[3] ?? "" },
    };
  match = /^INJECT \? TO (.+)$/u.exec(command);
  if (match)
    return {
      template: "inject",
      fields: { target: match[1] ?? "", messages: value(0) },
    };
  if (command === "CONFIG SET ? ?")
    return {
      template: "config_set",
      fields: { path: String(args[0] ?? ""), value: value(1) },
    };
  if (command === "CONFIG DELETE ?")
    return {
      template: "config_delete",
      fields: { path: String(args[0] ?? "") },
    };
  match = /^CREATE (TEMP )?BLOCK (\S+) \((.+)\)$/u.exec(command);
  if (match)
    return {
      template: "create_block",
      fields: {
        temp: !!match[1],
        name: match[2] ?? "",
        fields: match[3] ?? "",
      },
    };
  if (command === "EFFECT block_snapshot ?")
    return { template: "snapshot", fields: { block: String(args[0] ?? "") } };
  if (command === "EFFECT block_restore ? ?")
    return {
      template: "restore",
      fields: { block: String(args[0] ?? ""), snapshot: String(args[1] ?? "") },
    };
  if (command === "EFFECT sandbox_use ?")
    return { template: "sandbox_use", fields: { aliases: value(0) } };
  if (command === "EFFECT tool_call ?")
    return { template: "tool_call", fields: { calls: value(0) } };
  match = /^EFFECT (wake|start_turn|finish_turn|ready)$/u.exec(command);
  if (match) return { template: "effect", fields: { effect: match[1] ?? "" } };
  return {
    template: "custom",
    fields: { command, arguments: JSON.stringify(args, null, 2) },
  };
}

function applyTemplate(item: CustomInit) {
  const f = item.fields;
  const text = (key: string) => String(f[key] ?? "").trim();
  const json = (key: string) => jsonField(String(f[key] ?? ""));
  let command = "";
  let args: unknown[] = [];
  switch (item.template) {
    case "import_tool":
    case "import_message":
    case "import_sandbox":
      command = `IMPORT ${item.template === "import_tool" ? "TOOL" : item.template === "import_message" ? "MESSAGE" : "SANDBOX"} ${text("resource")} AS ${text("alias")}`;
      break;
    case "inject":
      command = `INJECT ? TO ${text("target")}`;
      args = [json("messages")];
      break;
    case "config_set":
      command = "CONFIG SET ? ?";
      args = [text("path"), json("value")];
      break;
    case "config_delete":
      command = "CONFIG DELETE ?";
      args = [text("path")];
      break;
    case "create_block":
      command = `CREATE ${f.temp ? "TEMP " : ""}BLOCK ${text("name")} (${text("fields")})`;
      break;
    case "snapshot":
      command = "EFFECT block_snapshot ?";
      args = [text("block")];
      break;
    case "restore":
      command = "EFFECT block_restore ? ?";
      args = [
        text("block"),
        /^\d+$/u.test(text("snapshot"))
          ? Number(text("snapshot"))
          : text("snapshot"),
      ];
      break;
    case "sandbox_use":
      command = "EFFECT sandbox_use ?";
      args = [json("aliases")];
      break;
    case "tool_call":
      command = "EFFECT tool_call ?";
      args = [json("calls")];
      break;
    case "effect":
      command = `EFFECT ${text("effect")}`;
      break;
    default:
      command = text("command");
      args = json("arguments") as unknown[];
  }
  item.command = command;
  item.argumentsText =
    args === undefined || args.some((value) => value === undefined)
      ? ""
      : JSON.stringify(args, null, 2);
}

function itemForStored(item: Record<string, unknown>): CustomInit {
  const command = typeof item.command === "string" ? item.command : "";
  const args = storedArguments(item);
  const inferred = inferTemplate(command, args);
  const custom: CustomInit = {
    template: inferred.template,
    fields: inferred.fields,
    command,
    argumentsText: JSON.stringify(args, null, 2),
  };
  applyTemplate(custom);
  return custom;
}

const emit = defineEmits<{ close: [] }>();
const store = useWorkbenchStore();
const { selectedAgent, selectedAgentReady, toolDefinitions } =
  storeToRefs(store);
const tab = ref<"tools" | "session" | "initialization">("tools");
const busy = ref(false);
const notice = ref("");

const tools = computed(() =>
  selectedAgent.value
    ? (toolDefinitions.value.get(selectedAgent.value) ?? [])
    : [],
);

const selectedToolAlias = ref("");
const selectedTool = computed(
  () =>
    tools.value.find((tool) => tool.alias === selectedToolAlias.value) ?? null,
);
const toolValues = ref<Record<string, unknown>>({});
const toolMessages = ref<AgentMessage[]>([]);
const toolError = ref("");

const schema = computed<Record<string, unknown>>(() =>
  isRecord(selectedTool.value?.input_schema)
    ? selectedTool.value.input_schema
    : {},
);
const schemaFields = computed<SchemaField[]>(() => {
  const properties = schema.value.properties;
  if (!isRecord(properties)) return [];
  const required = Array.isArray(schema.value.required)
    ? schema.value.required.filter(
        (value): value is string => typeof value === "string",
      )
    : [];
  return Object.entries(properties)
    .filter((entry): entry is [string, Record<string, unknown>] =>
      isRecord(entry[1]),
    )
    .map(([key, value]) => ({
      key,
      schema: value,
      required: required.includes(key),
    }));
});

function fieldType(field: SchemaField): string {
  const type = field.schema.type;
  return typeof type === "string"
    ? type
    : Array.isArray(type)
      ? String(type[0] ?? "")
      : "";
}

function fieldEnum(field: SchemaField): unknown[] | null {
  return Array.isArray(field.schema.enum) ? field.schema.enum : null;
}

function isComplex(field: SchemaField): boolean {
  return ["array", "object"].includes(fieldType(field));
}

function defaultValue(field: SchemaField): unknown {
  if ("default" in field.schema) return field.schema.default;
  switch (fieldType(field)) {
    case "boolean":
      return false;
    case "array":
      return "[]";
    case "object":
      return "{}";
    default:
      return "";
  }
}

function resetTool() {
  toolValues.value = {};
  toolMessages.value = [];
  toolError.value = "";
  for (const field of schemaFields.value)
    toolValues.value[field.key] = defaultValue(field);
}

watch(selectedToolAlias, resetTool);

function valueOf(field: SchemaField): string {
  const value = toolValues.value[field.key];
  return typeof value === "string"
    ? value
    : value === undefined || value === null
      ? ""
      : String(value);
}

function setValue(field: SchemaField, value: string | boolean) {
  toolValues.value[field.key] = value;
}

function visibleToolResult(result: unknown): AgentMessage[] {
  if (
    !isRecord(result) ||
    !isRecord(result.Array) ||
    !Array.isArray(result.Array.Message)
  )
    return [];
  return result.Array.Message.filter(isAgentMessage);
}

async function invokeTool() {
  if (!selectedTool.value || !selectedAgentReady.value || busy.value) return;
  toolError.value = "";
  toolMessages.value = [];
  const args: Record<string, unknown> = {};
  for (const field of schemaFields.value) {
    const raw = toolValues.value[field.key];
    if (field.required && (raw === undefined || raw === null || raw === "")) {
      toolError.value = `Required field: ${field.key}`;
      return;
    }
    if (raw === undefined || raw === null || raw === "") continue;
    if (isComplex(field)) {
      try {
        args[field.key] = JSON.parse(String(raw));
      } catch {
        toolError.value = `${field.key} must be valid JSON`;
        return;
      }
    } else if (fieldType(field) === "boolean") args[field.key] = raw === true;
    else if (["number", "integer"].includes(fieldType(field))) {
      const number = Number(raw);
      if (!Number.isFinite(number)) {
        toolError.value = `${field.key} must be a number`;
        return;
      }
      args[field.key] =
        fieldType(field) === "integer" ? Math.trunc(number) : number;
    } else args[field.key] = raw;
  }
  busy.value = true;
  try {
    const call = {
      id: `ui-tool-${Date.now()}`,
      tool_name: selectedTool.value.alias,
      arguments: JSON.stringify(args),
    };
    const result = await store.executeMclCommand("EFFECT tool_call ?", [
      [call],
    ]);
    toolMessages.value = visibleToolResult(result);
    if (!toolMessages.value.length)
      toolMessages.value = [
        { type: "system", content: JSON.stringify(result, null, 2) },
      ];
  } catch {
    toolError.value = "Tool call failed — see Activity logs";
  } finally {
    busy.value = false;
  }
}

const sessionMessages = ref<AgentMessage[]>([]);
const sessionIndex = ref<number | null>(null);
const sessionDraft = ref("");
const sessionNotice = ref("");
const sessionLoading = ref(false);

function setSessionIndex(index: number) {
  sessionIndex.value = index;
  const message = sessionMessages.value[index];
  sessionDraft.value = message ? JSON.stringify(message, null, 2) : "";
}

async function loadSession() {
  if (!selectedAgentReady.value || sessionLoading.value) return;
  sessionLoading.value = true;
  sessionNotice.value = "";
  try {
    const result = await store.executeMclCommand("GET session.conversation");
    const values =
      isRecord(result) &&
      isRecord(result.Array) &&
      Array.isArray(result.Array.Message)
        ? result.Array.Message.filter(isAgentMessage)
        : [];
    sessionMessages.value = values;
    if (values.length) setSessionIndex(Math.max(0, values.length - 1));
    else {
      sessionIndex.value = null;
      sessionDraft.value = '{\n  "type": "user",\n  "content": ""\n}';
    }
  } catch {
    sessionNotice.value = "Could not load session — see Activity logs";
  } finally {
    sessionLoading.value = false;
  }
}

function draftMessage(): AgentMessage | null {
  try {
    const value: unknown = JSON.parse(sessionDraft.value);
    if (!isAgentMessage(value)) throw new Error("invalid");
    return value;
  } catch {
    sessionNotice.value = "Message must be valid AgentMessage JSON";
    return null;
  }
}

async function sessionCommand(command: string, args: unknown[]) {
  if (!selectedAgentReady.value || busy.value) return;
  busy.value = true;
  sessionNotice.value = "";
  try {
    await store.executeMclCommand(command, args);
    await loadSession();
  } catch {
    sessionNotice.value = "Session update failed — see Activity logs";
  } finally {
    busy.value = false;
  }
}

async function appendSession() {
  const message = draftMessage();
  if (!message) return;
  await sessionCommand("INJECT ? TO session.conversation[-1]", [[message]]);
}

async function insertSession() {
  const message = draftMessage();
  if (!message) return;
  if (sessionIndex.value === null) return appendSession();
  await sessionCommand(
    `INJECT ? TO session.conversation[${sessionIndex.value}]`,
    [[message]],
  );
}

async function replaceSession() {
  const message = draftMessage();
  if (!message || sessionIndex.value === null) return;
  await sessionCommand(
    `INJECT ? TO session.conversation[${sessionIndex.value},${sessionIndex.value + 1}]`,
    [[message]],
  );
}

async function deleteSession() {
  if (sessionIndex.value === null) return;
  await sessionCommand(
    `INJECT [] TO session.conversation[${sessionIndex.value},${sessionIndex.value + 1}]`,
    [],
  );
}

async function snapshotSession() {
  await sessionCommand("EFFECT block_snapshot ?", ["session"]);
  sessionNotice.value = "Session snapshot requested.";
}

const customItems = ref<CustomInit[]>([]);
const initializationNotice = ref("");
const templateOptions = INIT_TEMPLATES.map((template) => ({
  value: template.id,
  label: template.label,
}));

async function loadInitialization() {
  if (!selectedAgentReady.value || busy.value) return;
  busy.value = true;
  initializationNotice.value = "";
  try {
    const result = await store.executeMclCommand("CONFIG GET");
    const config =
      isRecord(result) && isRecord(result.Config) ? result.Config : {};
    const initialization = isRecord(config.initialization)
      ? config.initialization
      : {};
    const custom = Array.isArray(initialization.custom)
      ? initialization.custom
      : [];
    customItems.value = custom.filter(isRecord).map(itemForStored);
  } catch {
    initializationNotice.value =
      "Could not load initialization — see Activity logs";
  } finally {
    busy.value = false;
  }
}

function addCustom() {
  const item: CustomInit = {
    template: "import_tool",
    fields: initFields("import_tool"),
    command: "",
    argumentsText: "",
  };
  applyTemplate(item);
  customItems.value.push(item);
}

function changeTemplate(item: CustomInit) {
  item.fields = initFields(item.template);
  applyTemplate(item);
}

function updateItem(item: CustomInit) {
  applyTemplate(item);
}

function removeCustom(index: number) {
  customItems.value.splice(index, 1);
}

async function saveInitialization() {
  if (!selectedAgentReady.value || busy.value) return;
  const custom: { command: string; arguments: unknown[] }[] = [];
  for (const [index, item] of customItems.value.entries()) {
    for (const field of templateFields(item.template)) {
      if (field.type === "boolean") continue;
      if (!String(item.fields[field.key] ?? "").trim()) {
        initializationNotice.value = `Command ${index + 1}: ${field.label} is required`;
        return;
      }
    }
    if (!item.command.trim()) {
      initializationNotice.value = `Command ${index + 1} is incomplete`;
      return;
    }
    let args: unknown;
    try {
      args = JSON.parse(item.argumentsText);
    } catch {
      initializationNotice.value = `Command ${index + 1} has invalid JSON arguments`;
      return;
    }
    if (!Array.isArray(args)) {
      initializationNotice.value = `Command ${index + 1} arguments must be a JSON array`;
      return;
    }
    const slots = [...item.command].filter((char) => char === "?").length;
    if (args.length !== slots) {
      initializationNotice.value = `Command ${index + 1} needs ${slots} argument(s) for its ? slots`;
      return;
    }
    custom.push({ command: item.command.trim(), arguments: args });
  }
  busy.value = true;
  initializationNotice.value = "";
  try {
    await store.executeMclCommand("CONFIG SET ? ?", [
      "initialization.custom",
      custom,
    ]);
    initializationNotice.value =
      "Custom initialization saved; it applies on the next Driver start.";
  } catch {
    initializationNotice.value =
      "Initialization was not saved — see Activity logs";
  } finally {
    busy.value = false;
  }
}

async function switchTab(next: "tools" | "session" | "initialization") {
  tab.value = next;
  notice.value = "";
  if (next === "tools" && !selectedToolAlias.value && tools.value[0])
    selectedToolAlias.value = tools.value[0].alias;
  if (next === "session") await loadSession();
  if (next === "initialization") await loadInitialization();
}

onMounted(() => {
  if (tools.value[0]) selectedToolAlias.value = tools.value[0].alias;
  void loadSession();
});
</script>

<template>
  <div class="advanced-backdrop" @click.self="emit('close')">
    <section
      class="advanced-panel"
      role="dialog"
      aria-modal="true"
      aria-label="Advanced Driver tools"
    >
      <header class="advanced-header">
        <div>
          <span class="eyebrow">Coder Driver</span>
          <h2>Advanced</h2>
        </div>
        <button
          class="icon-button"
          type="button"
          title="Close"
          @click="emit('close')"
        >
          <X :size="17" />
        </button>
      </header>
      <div class="advanced-tabs" role="tablist">
        <button
          v-for="item in [
            { id: 'tools', label: 'Tools' },
            { id: 'session', label: 'Session' },
            { id: 'initialization', label: 'Initialization' },
          ] as const"
          :key="item.id"
          type="button"
          :class="{ active: tab === item.id }"
          @click="switchTab(item.id)"
        >
          {{ item.label }}
        </button>
      </div>
      <p v-if="notice" class="advanced-notice">{{ notice }}</p>

      <div v-if="tab === 'tools'" class="advanced-split">
        <aside class="advanced-list">
          <button
            v-for="tool in tools"
            :key="tool.alias"
            type="button"
            :class="[
              'advanced-list-item',
              { active: tool.alias === selectedToolAlias },
            ]"
            @click="selectedToolAlias = tool.alias"
          >
            <strong>{{ tool.alias }}</strong>
            <span>{{ tool.description || "No description." }}</span>
          </button>
          <p v-if="!tools.length" class="advanced-empty">No tools loaded.</p>
        </aside>
        <div v-if="selectedTool" class="advanced-editor">
          <div class="advanced-section-heading">
            <div>
              <strong>{{ selectedTool.alias }}</strong>
              <span>{{ selectedTool.path }}</span>
            </div>
            <button
              type="button"
              class="secondary-button"
              :disabled="busy"
              @click="invokeTool"
            >
              {{ busy ? "Calling…" : "Call tool" }}
            </button>
          </div>
          <p class="advanced-hint">{{ selectedTool.description }}</p>
          <div class="schema-grid">
            <label
              v-for="field in schemaFields"
              :key="field.key"
              class="schema-field"
            >
              <span>
                {{ field.key }}
                <em v-if="field.required">required</em>
              </span>
              <select
                v-if="fieldEnum(field)"
                class="field-input"
                :value="valueOf(field)"
                @change="
                  setValue(field, ($event.target as HTMLSelectElement).value)
                "
              >
                <option
                  v-for="value in fieldEnum(field)!"
                  :key="String(value)"
                  :value="String(value)"
                >
                  {{ value }}
                </option>
              </select>
              <input
                v-else-if="fieldType(field) === 'boolean'"
                type="checkbox"
                :checked="toolValues[field.key] === true"
                @change="
                  setValue(field, ($event.target as HTMLInputElement).checked)
                "
              />
              <textarea
                v-else-if="isComplex(field)"
                class="field-input schema-json"
                :value="valueOf(field)"
                spellcheck="false"
                @input="
                  setValue(field, ($event.target as HTMLTextAreaElement).value)
                "
              ></textarea>
              <input
                v-else
                class="field-input"
                :type="
                  ['number', 'integer'].includes(fieldType(field))
                    ? 'number'
                    : 'text'
                "
                :value="valueOf(field)"
                :placeholder="
                  typeof field.schema.description === 'string'
                    ? field.schema.description
                    : ''
                "
                @input="
                  setValue(field, ($event.target as HTMLInputElement).value)
                "
              />
              <small v-if="typeof field.schema.description === 'string'">{{
                field.schema.description
              }}</small>
            </label>
          </div>
          <p v-if="!schemaFields.length" class="advanced-empty">
            This tool has no input schema; call it without arguments.
          </p>
          <p v-if="toolError" class="advanced-error">{{ toolError }}</p>
          <div v-if="toolMessages.length" class="advanced-results">
            <article
              v-for="(message, index) in toolMessages"
              :key="index"
              :class="[
                'advanced-result',
                { failed: message.type === 'tool_call' && message.failed },
              ]"
            >
              <strong>{{ message.type }}</strong>
              <pre v-if="'content' in message && message.content">{{
                message.content
              }}</pre>
              <pre v-else>{{ JSON.stringify(message, null, 2) }}</pre>
              <div
                v-if="message.type === 'tool_call' && message.data?.length"
                class="advanced-images"
              >
                <img
                  v-for="(image, imageIndex) in message.data"
                  :key="imageIndex"
                  :src="imageSource(image)"
                  alt="Tool result"
                />
              </div>
            </article>
          </div>
        </div>
      </div>

      <div v-else-if="tab === 'session'" class="advanced-session">
        <div class="advanced-section-heading">
          <div>
            <strong>session.conversation</strong>
            <span>Live Driver context block</span>
          </div>
          <div class="advanced-actions">
            <button
              type="button"
              class="secondary-button"
              :disabled="sessionLoading"
              @click="loadSession"
            >
              {{ sessionLoading ? "Loading…" : "Reload" }}
            </button>
            <button
              type="button"
              class="secondary-button"
              :disabled="busy"
              @click="snapshotSession"
            >
              Snapshot
            </button>
          </div>
        </div>
        <p class="advanced-hint advanced-warning">
          Editing this block changes the Driver’s live inference context
          immediately. Message edits are not persisted history.
        </p>
        <div class="advanced-split advanced-session-split">
          <aside class="advanced-list">
            <button
              v-for="(message, index) in sessionMessages"
              :key="index"
              type="button"
              :class="[
                'advanced-list-item',
                { active: sessionIndex === index },
              ]"
              @click="setSessionIndex(index)"
            >
              <strong>#{{ index }} · {{ message.type }}</strong>
              <span>{{
                ("content" in message && message.content) ||
                JSON.stringify(message).slice(0, 90)
              }}</span>
            </button>
            <p v-if="!sessionMessages.length" class="advanced-empty">
              Session is empty.
            </p>
          </aside>
          <div class="advanced-editor">
            <textarea
              v-model="sessionDraft"
              class="field-input schema-json advanced-session-json"
              spellcheck="false"
              aria-label="Selected session message JSON"
            ></textarea>
            <div class="advanced-actions">
              <button
                type="button"
                class="secondary-button"
                :disabled="busy"
                @click="insertSession"
              >
                Insert before
              </button>
              <button
                type="button"
                class="secondary-button"
                :disabled="busy"
                @click="appendSession"
              >
                Append
              </button>
              <button
                type="button"
                class="secondary-button"
                :disabled="busy"
                @click="replaceSession"
              >
                Replace
              </button>
              <button
                type="button"
                class="secondary-button advanced-danger"
                :disabled="busy"
                @click="deleteSession"
              >
                Delete
              </button>
            </div>
            <p v-if="sessionNotice" class="advanced-notice">
              {{ sessionNotice }}
            </p>
          </div>
        </div>
      </div>

      <div v-else class="advanced-init">
        <div class="advanced-section-heading">
          <div>
            <strong>initialization.custom</strong>
            <span>Commands run after the Driver defaults on startup</span>
          </div>
          <div class="advanced-actions">
            <button
              type="button"
              class="secondary-button"
              :disabled="busy"
              @click="addCustom"
            >
              Add command
            </button>
            <button
              type="button"
              class="primary-button"
              :disabled="busy"
              @click="saveInitialization"
            >
              {{ busy ? "Saving…" : "Save initialization" }}
            </button>
          </div>
        </div>
        <p class="advanced-hint">
          Pick an MCL command and fill its typed fields. Commands run after the
          Driver defaults on the next start.
        </p>
        <div class="advanced-custom-list">
          <article
            v-for="(item, index) in customItems"
            :key="index"
            class="advanced-custom-row"
          >
            <div class="advanced-custom-head">
              <FormSelect
                v-model="item.template"
                :options="templateOptions"
                @change="changeTemplate(item)"
              />
              <button
                type="button"
                class="advanced-remove"
                @click="removeCustom(index)"
              >
                Remove
              </button>
            </div>
            <div class="init-fields">
              <label
                v-for="field in templateFields(item.template)"
                :key="field.key"
                :class="{ 'init-wide': field.wide }"
              >
                <span>{{ field.label }}</span>
                <input
                  v-if="field.type === 'boolean'"
                  type="checkbox"
                  :checked="item.fields[field.key] === true"
                  @change="
                    item.fields[field.key] = (
                      $event.target as HTMLInputElement
                    ).checked;
                    updateItem(item);
                  "
                />
                <textarea
                  v-else-if="field.type === 'json'"
                  class="field-input schema-json"
                  :value="String(item.fields[field.key] ?? '')"
                  :placeholder="field.placeholder"
                  spellcheck="false"
                  @input="
                    item.fields[field.key] = (
                      $event.target as HTMLTextAreaElement
                    ).value;
                    updateItem(item);
                  "
                ></textarea>
                <input
                  v-else
                  class="field-input"
                  :value="String(item.fields[field.key] ?? '')"
                  :placeholder="field.placeholder"
                  @input="
                    item.fields[field.key] = (
                      $event.target as HTMLInputElement
                    ).value;
                    updateItem(item);
                  "
                />
              </label>
            </div>
            <code class="init-preview"
              >{{ item.command }} {{ item.argumentsText }}</code
            >
          </article>
          <p v-if="!customItems.length" class="advanced-empty">
            No custom commands.
          </p>
        </div>
        <p v-if="initializationNotice" class="advanced-notice">
          {{ initializationNotice }}
        </p>
      </div>
    </section>
  </div>
</template>
