<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { storeToRefs } from "pinia";
import { isRecord, isAgentMessage, type AgentMessage } from "@/types/protocol";
import { useWorkbenchStore } from "@/stores/workbench";
import { imageSource } from "@/lib/images";
import { X } from "lucide-vue-next";
import FormSelect from "@/components/FormSelect.vue";
import ToolSchemaField from "@/components/ToolSchemaField.vue";

interface SchemaField {
  key: string;
  schema: Record<string, unknown>;
  required: boolean;
}

interface InitArg {
  kind: "json" | "text" | "number" | "select";
  label: string;
  inline?: boolean;
  placeholder?: string;
  options?: { value: string; label: string }[];
}
interface InitTemplate {
  id: string;
  label: string;
  template: string;
  args: InitArg[];
}
interface CustomInit {
  template: string;
  values: string[];
  active: number | null;
  command: string;
  argumentsText: string;
}

const INIT_TEMPLATES: InitTemplate[] = [
  {
    id: "import_tool",
    label: "IMPORT TOOL",
    template: "IMPORT TOOL <arg1> AS <arg2>",
    args: [
      {
        kind: "text",
        label: "resource",
        inline: true,
        placeholder: "tool:local/read-file:latest",
      },
      { kind: "text", label: "alias", inline: true, placeholder: "read_file" },
    ],
  },
  {
    id: "import_message",
    label: "IMPORT MESSAGE",
    template: "IMPORT MESSAGE <arg1> AS <arg2>",
    args: [
      {
        kind: "text",
        label: "resource",
        inline: true,
        placeholder: "prompt:local/coder-soul:latest",
      },
      { kind: "text", label: "alias", inline: true, placeholder: "soul" },
    ],
  },
  {
    id: "import_sandbox",
    label: "IMPORT SANDBOX",
    template: "IMPORT SANDBOX <arg1> AS <arg2>",
    args: [
      {
        kind: "text",
        label: "resource",
        inline: true,
        placeholder: "sandbox:local/read-only:latest",
      },
      { kind: "text", label: "alias", inline: true, placeholder: "read_only" },
    ],
  },
  {
    id: "inject",
    label: "INJECT messages",
    template: "INJECT <arg1> TO <arg2>",
    args: [
      {
        kind: "json",
        label: "messages",
        placeholder: '[{"type":"system","content":""}]',
      },
      {
        kind: "text",
        label: "target",
        inline: true,
        placeholder: "session.conversation[-1]",
      },
    ],
  },
  {
    id: "config_set",
    label: "CONFIG SET",
    template: "CONFIG SET <arg1> <arg2>",
    args: [
      { kind: "text", label: "path", placeholder: "compaction.threshold" },
      { kind: "json", label: "value", placeholder: "0.8" },
    ],
  },
  {
    id: "config_delete",
    label: "CONFIG DELETE",
    template: "CONFIG DELETE <arg1>",
    args: [{ kind: "text", label: "path", placeholder: "custom.option" }],
  },
  {
    id: "create_block",
    label: "CREATE BLOCK",
    template: "CREATE BLOCK <arg1> (<arg2>)",
    args: [
      { kind: "text", label: "name", inline: true, placeholder: "session" },
      {
        kind: "text",
        label: "fields",
        inline: true,
        placeholder: "conversation",
      },
    ],
  },
  {
    id: "create_temp_block",
    label: "CREATE TEMP BLOCK",
    template: "CREATE TEMP BLOCK <arg1> (<arg2>)",
    args: [
      { kind: "text", label: "name", inline: true, placeholder: "entry" },
      { kind: "text", label: "fields", inline: true, placeholder: "pending" },
    ],
  },
  {
    id: "snapshot",
    label: "EFFECT block_snapshot",
    template: "EFFECT block_snapshot <arg1>",
    args: [
      {
        kind: "select",
        label: "block",
        options: [{ value: "session", label: "session" }],
      },
    ],
  },
  {
    id: "restore",
    label: "EFFECT block_restore",
    template: "EFFECT block_restore <arg1> <arg2>",
    args: [
      {
        kind: "select",
        label: "block",
        options: [{ value: "session", label: "session" }],
      },
      { kind: "number", label: "snapshot id", placeholder: "1" },
    ],
  },
  {
    id: "sandbox_use",
    label: "EFFECT sandbox_use",
    template: "EFFECT sandbox_use <arg1>",
    args: [{ kind: "json", label: "aliases", placeholder: '["read_only"]' }],
  },
  {
    id: "tool_call",
    label: "EFFECT tool_call",
    template: "EFFECT tool_call <arg1>",
    args: [{ kind: "json", label: "tool calls", placeholder: "[]" }],
  },
  {
    id: "effect",
    label: "EFFECT",
    template: "EFFECT <arg1>",
    args: [
      {
        kind: "select",
        label: "effect",
        inline: true,
        options: ["wake", "start_turn", "finish_turn", "ready"].map(
          (value) => ({ value, label: value }),
        ),
      },
    ],
  },
  {
    id: "custom",
    label: "Custom command",
    template: "<arg1>",
    args: [
      {
        kind: "text",
        label: "command",
        inline: true,
        placeholder: "GET session.conversation",
      },
      { kind: "json", label: "arguments", placeholder: "[]" },
    ],
  },
];

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

function itemsSchema(field: SchemaField): Record<string, unknown> | null {
  return isRecord(field.schema.items) ? field.schema.items : null;
}

function itemType(field: SchemaField): string {
  const type = itemsSchema(field)?.type;
  return typeof type === "string" ? type : "string";
}

function itemEnum(field: SchemaField): unknown[] | null {
  const items = itemsSchema(field);
  return items && Array.isArray(items.enum) ? items.enum : null;
}

function itemIsJson(field: SchemaField): boolean {
  return ["array", "object"].includes(itemType(field));
}

function schemaDefault(schema: Record<string, unknown>): unknown {
  if ("default" in schema) return schema.default;
  if ("const" in schema) return schema.const;
  if (Array.isArray(schema.oneOf) && isRecord(schema.oneOf[0])) {
    const variant = schema.oneOf[0];
    const value: Record<string, unknown> = {};
    if (
      isRecord(variant.properties) &&
      isRecord(variant.properties.transport) &&
      "const" in variant.properties.transport
    )
      value.transport = variant.properties.transport.const;
    return value;
  }
  switch (typeof schema.type === "string" ? schema.type : "") {
    case "boolean":
      return false;
    case "array":
      return [];
    case "object":
      return {};
    default:
      return "";
  }
}

function defaultValue(field: SchemaField): unknown {
  return schemaDefault(field.schema);
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

function setValue(field: SchemaField, value: unknown) {
  toolValues.value[field.key] = value;
}

function enumOptions(values: unknown[]) {
  return values.map((value) => ({
    value: String(value),
    label: String(value),
  }));
}

function setEnumValue(field: SchemaField, value: string) {
  toolValues.value[field.key] =
    fieldEnum(field)?.find((option) => String(option) === value) ?? value;
}

function arrayValue(field: SchemaField): unknown[] {
  const value = toolValues.value[field.key];
  return Array.isArray(value) ? value : [];
}

function arrayItemValue(field: SchemaField, index: number): string {
  const value = arrayValue(field)[index];
  return typeof value === "string"
    ? value
    : value === undefined || value === null
      ? ""
      : String(value);
}

function setArrayItem(field: SchemaField, index: number, value: unknown) {
  const values = [...arrayValue(field)];
  values[index] = value;
  toolValues.value[field.key] = values;
}

function addArrayItem(field: SchemaField) {
  const values = [...arrayValue(field)];
  const enumValue = itemEnum(field)?.[0];
  if (enumValue !== undefined) values.push(enumValue);
  else if (itemType(field) === "boolean") values.push(false);
  else if (itemIsJson(field))
    values.push(itemType(field) === "array" ? "[]" : "{}");
  else values.push("");
  toolValues.value[field.key] = values;
}

function removeArrayItem(field: SchemaField, index: number) {
  const values = [...arrayValue(field)];
  values.splice(index, 1);
  toolValues.value[field.key] = values;
}

function enumArrayItem(field: SchemaField, index: number, value: string) {
  setArrayItem(
    field,
    index,
    itemEnum(field)?.find((option) => String(option) === value) ?? value,
  );
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

function validateSchemaValue(
  schema: Record<string, unknown>,
  value: unknown,
  path: string,
): string | null {
  if (Array.isArray(schema.oneOf) && isRecord(value)) {
    const variant = schema.oneOf.find(
      (item) =>
        isRecord(item) &&
        (!isRecord(item.properties) ||
          !isRecord(item.properties.transport) ||
          !("const" in item.properties.transport) ||
          value.transport === item.properties.transport.const),
    );
    if (isRecord(variant)) return validateSchemaValue(variant, value, path);
  }
  if (
    typeof schema.type === "string" &&
    schema.type !== typeof value &&
    !(schema.type === "integer" && typeof value === "number")
  ) {
    return `${path} has the wrong type`;
  }
  if (typeof value === "string") {
    if (typeof schema.minLength === "number" && value.length < schema.minLength)
      return `${path} is too short`;
    if (typeof schema.maxLength === "number" && value.length > schema.maxLength)
      return `${path} is too long`;
    if (
      typeof schema.pattern === "string" &&
      !new RegExp(schema.pattern).test(value)
    )
      return `${path} does not match the required pattern`;
  }
  if (Array.isArray(value) && isRecord(schema.items)) {
    for (const [index, item] of value.entries()) {
      const issue = validateSchemaValue(
        schema.items,
        item,
        `${path}[${index}]`,
      );
      if (issue) return issue;
    }
  }
  if (isRecord(value) && isRecord(schema.properties)) {
    const required = Array.isArray(schema.required)
      ? schema.required.filter((key): key is string => typeof key === "string")
      : [];
    for (const key of required) {
      const child = value[key];
      if (child === undefined || child === null || child === "")
        return `${path}.${key} is required`;
    }
    for (const [key, child] of Object.entries(schema.properties)) {
      if (!isRecord(child) || !(key in value)) continue;
      const issue = validateSchemaValue(child, value[key], `${path}.${key}`);
      if (issue) return issue;
    }
  }
  return null;
}

const toolArgumentsPreview = computed(() => {
  const args: Record<string, unknown> = {};
  for (const field of schemaFields.value)
    args[field.key] = toolValues.value[field.key] ?? null;
  return JSON.stringify(args, null, 2);
});

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
    const issue = validateSchemaValue(field.schema, raw, field.key);
    if (issue) {
      toolError.value = issue;
      return;
    }
    args[field.key] = raw;
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
    toolError.value = "Tool call failed";
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
    sessionNotice.value = "Could not load session";
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
    sessionNotice.value = "Session update failed";
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

function tryParseJson(value: string): unknown {
  try {
    return JSON.parse(value);
  } catch {
    return undefined;
  }
}

function readableValue(value: unknown, depth = 0): string {
  const indent = "  ".repeat(depth);
  if (Array.isArray(value)) {
    if (!value.length) return "[]";
    return `[\n${value
      .map(
        (item) => `${"  ".repeat(depth + 1)}${readableValue(item, depth + 1)}`,
      )
      .join(",\n")}\n${indent}]`;
  }
  if (isRecord(value)) {
    const entries = Object.entries(value);
    if (!entries.length) return "{}";
    return `{\n${entries
      .map(
        ([key, item]) =>
          `${"  ".repeat(depth + 1)}${key}: ${readableValue(item, depth + 1)}`,
      )
      .join(",\n")}\n${indent}}`;
  }
  if (typeof value === "string") return value;
  return String(value);
}

function readableArguments(value: string): string {
  const parsed = tryParseJson(value);
  return parsed === undefined ? value : readableValue(parsed);
}

const sessionPreview = computed<AgentMessage | null>(() => {
  const parsed = tryParseJson(sessionDraft.value);
  if (isAgentMessage(parsed)) return parsed;
  if (sessionIndex.value !== null)
    return sessionMessages.value[sessionIndex.value] ?? null;
  return null;
});

const customItems = ref<CustomInit[]>([]);
const initializationNotice = ref("");
const templateOptions = INIT_TEMPLATES.map((template) => ({
  value: template.id,
  label: template.label,
}));

function templateById(id: string): InitTemplate {
  return (
    INIT_TEMPLATES.find((template) => template.id === id) ?? INIT_TEMPLATES[0]!
  );
}

function defaultValues(template: InitTemplate): string[] {
  return template.args.map((arg) => {
    if (arg.kind === "select") return arg.options?.[0]?.value ?? "";
    if (arg.kind === "json")
      return ["inject", "sandbox_use", "tool_call", "custom"].includes(
        template.id,
      )
        ? "[]"
        : "null";
    return "";
  });
}

function buildInit(item: CustomInit) {
  const template = templateById(item.template);
  if (template.id === "custom") {
    item.command = String(item.values[0] ?? "").trim();
    try {
      const parsed = JSON.parse(String(item.values[1] ?? "[]"));
      item.argumentsText = Array.isArray(parsed)
        ? JSON.stringify(parsed, null, 2)
        : "";
    } catch {
      item.argumentsText = "";
    }
    return;
  }
  const args: unknown[] = [];
  let command = template.template;
  let valid = true;
  template.args.forEach((arg, index) => {
    const raw = String(item.values[index] ?? "");
    if (arg.inline) {
      command = command.replace(`<arg${index + 1}>`, raw);
      return;
    }
    command = command.replace(`<arg${index + 1}>`, "?");
    try {
      if (arg.kind === "json") args.push(JSON.parse(raw));
      else if (arg.kind === "number") {
        const number = Number(raw);
        if (!Number.isFinite(number)) throw new Error("not a number");
        args.push(number);
      } else args.push(raw);
    } catch {
      valid = false;
      args.push(undefined);
    }
  });
  item.command = command.trim();
  item.argumentsText = valid ? JSON.stringify(args, null, 2) : "";
}

function itemForStored(item: Record<string, unknown>): CustomInit {
  const command = typeof item.command === "string" ? item.command : "";
  const args = Array.isArray(item.arguments) ? item.arguments : [];
  let template = "custom";
  let values = [command, JSON.stringify(args, null, 2)];
  let match = /^IMPORT (TOOL|MESSAGE|SANDBOX)\s+(\S+)\s+AS\s+(\S+)/u.exec(
    command,
  );
  if (match) {
    template =
      match[1] === "TOOL"
        ? "import_tool"
        : match[1] === "MESSAGE"
          ? "import_message"
          : "import_sandbox";
    values = [match[2] ?? "", match[3] ?? ""];
  } else if ((match = /^INJECT \? TO (.+)$/u.exec(command))) {
    template = "inject";
    values = [JSON.stringify(args[0] ?? [], null, 2), match[1] ?? ""];
  } else if (command === "CONFIG SET ? ?") {
    template = "config_set";
    values = [String(args[0] ?? ""), JSON.stringify(args[1] ?? null, null, 2)];
  } else if (command === "CONFIG DELETE ?") {
    template = "config_delete";
    values = [String(args[0] ?? "")];
  } else if ((match = /^CREATE (TEMP )?BLOCK (\S+) \((.+)\)$/u.exec(command))) {
    template = match[1] ? "create_temp_block" : "create_block";
    values = [match[2] ?? "", match[3] ?? ""];
  } else if (command === "EFFECT block_snapshot ?") {
    template = "snapshot";
    values = [String(args[0] ?? "session")];
  } else if (command === "EFFECT block_restore ? ?") {
    template = "restore";
    values = [String(args[0] ?? "session"), String(args[1] ?? "")];
  } else if (command === "EFFECT sandbox_use ?") {
    template = "sandbox_use";
    values = [JSON.stringify(args[0] ?? [], null, 2)];
  } else if (command === "EFFECT tool_call ?") {
    template = "tool_call";
    values = [JSON.stringify(args[0] ?? [], null, 2)];
  } else if (
    (match = /^EFFECT (wake|start_turn|finish_turn|ready)$/u.exec(command))
  ) {
    template = "effect";
    values = [match[1] ?? "wake"];
  }
  const custom: CustomInit = {
    template,
    values,
    active: null,
    command,
    argumentsText: JSON.stringify(args, null, 2),
  };
  buildInit(custom);
  return custom;
}

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
    initializationNotice.value = "Could not load initialization";
  } finally {
    busy.value = false;
  }
}

function addCustom() {
  const template = templateById("import_tool");
  const item: CustomInit = {
    template: template.id,
    values: defaultValues(template),
    active: null,
    command: "",
    argumentsText: "",
  };
  buildInit(item);
  customItems.value.push(item);
}

function changeTemplate(item: CustomInit) {
  const template = templateById(item.template);
  item.values = defaultValues(template);
  item.active = null;
  buildInit(item);
}

function toggleArg(item: CustomInit, index: number) {
  item.active = item.active === index ? null : index;
}

function setArgValue(item: CustomInit, index: number, value: string) {
  item.values[index] = value;
  buildInit(item);
}

function removeCustom(index: number) {
  customItems.value.splice(index, 1);
}

function commandSegments(item: CustomInit) {
  return templateById(item.template)
    .template.split(/(<arg\d+>)/u)
    .filter((part) => part.length > 0)
    .map((part) => {
      const match = /^<arg(\d+)>$/u.exec(part);
      return match
        ? { text: "", arg: Number(match[1]) }
        : { text: part, arg: null };
    });
}

async function saveInitialization() {
  if (!selectedAgentReady.value || busy.value) return;
  const custom: { command: string; arguments: unknown[] }[] = [];
  for (const [index, item] of customItems.value.entries()) {
    const template = templateById(item.template);
    for (const [argIndex, arg] of template.args.entries()) {
      if (!String(item.values[argIndex] ?? "").trim()) {
        initializationNotice.value = `Command ${index + 1}: arg${argIndex + 1} (${arg.label}) is required`;
        return;
      }
    }
    buildInit(item);
    if (!item.argumentsText) {
      initializationNotice.value = `Command ${index + 1} has an invalid JSON argument`;
      return;
    }
    const args = JSON.parse(item.argumentsText) as unknown[];
    if (!item.command) {
      initializationNotice.value = `Command ${index + 1} is incomplete`;
      return;
    }
    const slots = [...item.command].filter((char) => char === "?").length;
    if (args.length !== slots) {
      initializationNotice.value = `Command ${index + 1} needs ${slots} argument(s) for its ? slots`;
      return;
    }
    custom.push({ command: item.command, arguments: args });
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
    initializationNotice.value = "Initialization was not saved";
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
              <ToolSchemaField
                :schema="field.schema"
                :model-value="toolValues[field.key]"
                @update:model-value="setValue(field, $event)"
              />
              <small v-if="typeof field.schema.description === 'string'">{{
                field.schema.description
              }}</small>
            </label>
          </div>
          <div v-if="schemaFields.length" class="tool-args-preview">
            <div class="tool-args-preview-head">
              <span>Arguments JSON</span>
              <span>live</span>
            </div>
            <pre>{{ toolArgumentsPreview }}</pre>
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
            <div v-if="sessionPreview" class="session-preview">
              <div class="session-preview-meta">
                <span>{{ sessionPreview.type }}</span>
                <span
                  v-if="
                    'request_id' in sessionPreview && sessionPreview.request_id
                  "
                >
                  {{ sessionPreview.request_id }}
                </span>
              </div>
              <template
                v-if="'reasoning' in sessionPreview && sessionPreview.reasoning"
              >
                <span class="session-preview-label">reasoning</span>
                <pre class="session-preview-text">{{
                  sessionPreview.reasoning
                }}</pre>
              </template>
              <template
                v-if="'content' in sessionPreview && sessionPreview.content"
              >
                <span class="session-preview-label">content</span>
                <pre class="session-preview-text">{{
                  sessionPreview.content
                }}</pre>
              </template>
              <template v-if="sessionPreview.type === 'inject'">
                <span class="session-preview-label">messages</span>
                <pre class="session-preview-text">{{
                  readableValue(sessionPreview.messages)
                }}</pre>
              </template>
            </div>
            <details class="session-raw">
              <summary>Raw JSON</summary>
              <textarea
                v-model="sessionDraft"
                class="field-input schema-json advanced-session-json"
                spellcheck="false"
                aria-label="Selected session message JSON"
              ></textarea>
            </details>
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
            <div class="init-template">
              <template
                v-for="(segment, segmentIndex) in commandSegments(item)"
                :key="segmentIndex"
              >
                <span>{{ segment.text }}</span>
                <button
                  v-if="segment.arg"
                  type="button"
                  class="init-arg-chip"
                  :class="{ active: item.active === segment.arg - 1 }"
                  @click="toggleArg(item, segment.arg - 1)"
                >
                  arg{{ segment.arg }}
                </button>
              </template>
            </div>
            <div class="init-args">
              <div
                v-for="(arg, argIndex) in templateById(item.template).args"
                :key="argIndex"
                class="init-arg"
                :class="{ active: item.active === argIndex }"
              >
                <button
                  type="button"
                  class="init-arg-trigger"
                  @click="toggleArg(item, argIndex)"
                >
                  <span class="init-arg-chip">arg{{ argIndex + 1 }}</span>
                  <span>{{ arg.label }}</span>
                  <span class="init-arg-chevron">{{
                    item.active === argIndex ? "−" : "+"
                  }}</span>
                </button>
                <div v-if="item.active === argIndex" class="init-arg-editor">
                  <FormSelect
                    v-if="arg.kind === 'select'"
                    :model-value="item.values[argIndex] ?? ''"
                    :options="arg.options ?? []"
                    @change="setArgValue(item, argIndex, $event)"
                  />
                  <textarea
                    v-else-if="arg.kind === 'json'"
                    class="field-input schema-json init-arg-json"
                    :value="item.values[argIndex]"
                    :placeholder="arg.placeholder"
                    spellcheck="false"
                    @input="
                      setArgValue(
                        item,
                        argIndex,
                        ($event.target as HTMLTextAreaElement).value,
                      )
                    "
                  ></textarea>
                  <input
                    v-else
                    class="field-input"
                    :type="arg.kind === 'number' ? 'number' : 'text'"
                    :value="item.values[argIndex]"
                    :placeholder="arg.placeholder"
                    @input="
                      setArgValue(
                        item,
                        argIndex,
                        ($event.target as HTMLInputElement).value,
                      )
                    "
                  />
                </div>
              </div>
            </div>
            <pre class="init-preview"
              >{{ item.command }} {{
                readableArguments(item.argumentsText)
              }}</pre>
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
