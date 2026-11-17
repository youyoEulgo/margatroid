<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { isRecord } from '@/types/protocol';
import FormSelect from '@/components/FormSelect.vue';

defineOptions({ name: 'ToolSchemaField' });

const props = defineProps<{
  schema: Record<string, unknown>;
  modelValue: unknown;
}>();
const emit = defineEmits<{ 'update:modelValue': [unknown] }>();

type Schema = Record<string, unknown>;

const variants = computed<Schema[]>(() => {
  const values = Array.isArray(props.schema.oneOf)
    ? props.schema.oneOf
    : Array.isArray(props.schema.anyOf)
      ? props.schema.anyOf
      : [];
  return values.filter(isRecord);
});

const chosen = ref(0);

function variantValue(schema: Schema): string {
  if (typeof schema.const === 'string') return schema.const;
  if (Array.isArray(schema.enum) && schema.enum.length === 1) return String(schema.enum[0]);
  return '';
}

function variantLabel(schema: Schema, index: number): string {
  if (typeof schema.title === 'string' && schema.title) return schema.title;
  const value = variantValue(schema);
  if (value) return value;
  if (
    isRecord(schema.properties) &&
    isRecord(schema.properties.transport) &&
    'const' in schema.properties.transport
  )
    return String(schema.properties.transport.const);
  return `Variant ${index + 1}`;
}

const variantOptions = computed(() =>
  variants.value.map((schema, index) => ({
    value: String(index),
    label: variantLabel(schema, index),
  })),
);

const selectedVariant = computed<Schema | null>(() => variants.value[chosen.value] ?? null);

watch(
  () => props.modelValue,
  (value) => {
    if (!variants.value.length) return;
    const index = variants.value.findIndex((schema) => {
      const expected = variantValue(schema);
      return expected ? isRecord(value) && value.transport === expected : false;
    });
    if (index >= 0) chosen.value = index;
  },
  { immediate: true, deep: true },
);

function isObjectSchema(schema: Schema): boolean {
  return (
    schema.type === 'object' || isRecord(schema.properties) || isRecord(schema.additionalProperties)
  );
}

function typeOf(schema: Schema): string {
  return typeof schema.type === 'string' ? schema.type : '';
}

function enumValues(schema: Schema): unknown[] | null {
  return Array.isArray(schema.enum) ? schema.enum : null;
}

function defaultValue(schema: Schema): unknown {
  if ('default' in schema) return schema.default;
  if ('const' in schema) return schema.const;
  if (variants.value.length) {
    return defaultForVariant(variants.value[0]!);
  }
  if (enumValues(schema)?.length) return enumValues(schema)![0];
  switch (typeOf(schema)) {
    case 'boolean':
      return false;
    case 'array':
      return [];
    case 'object':
      return objectDefaults(schema);
    default:
      return '';
  }
}

function objectDefaults(schema: Schema): Record<string, unknown> {
  const value: Record<string, unknown> = {};
  if (isRecord(schema.properties)) {
    for (const [key, child] of Object.entries(schema.properties))
      if (isRecord(child) && 'const' in child) value[key] = child.const;
  }
  return value;
}

function defaultForVariant(schema: Schema): unknown {
  const value = objectDefaults(schema);
  if ('const' in schema) return schema.const;
  if (isRecord(schema.properties) && 'transport' in schema.properties) {
    const transport = schema.properties.transport;
    if (isRecord(transport) && 'const' in transport) value.transport = transport.const;
  }
  return value;
}

function chooseVariant(value: string) {
  const index = Number(value);
  chosen.value = index;
  emit('update:modelValue', defaultForVariant(variants.value[index] ?? {}));
}

const objectValue = computed<Record<string, unknown>>(() =>
  isRecord(props.modelValue) ? props.modelValue : {},
);

const properties = computed(() => {
  const source = selectedVariant.value ?? props.schema;
  return isRecord(source.properties)
    ? Object.entries(source.properties).filter((entry): entry is [string, Schema] =>
        isRecord(entry[1]),
      )
    : [];
});

const required = computed(() => {
  const source = selectedVariant.value ?? props.schema;
  return new Set(
    Array.isArray(source.required)
      ? source.required.filter((value): value is string => typeof value === 'string')
      : [],
  );
});

const additionalSchema = computed<Schema | null>(() => {
  const source = selectedVariant.value ?? props.schema;
  return isRecord(source.additionalProperties) ? source.additionalProperties : null;
});

const mapValue = computed<Record<string, unknown>>(() =>
  isRecord(props.modelValue) ? props.modelValue : {},
);

function emitObject(value: Record<string, unknown>) {
  const source = selectedVariant.value ?? props.schema;
  if (isRecord(source.properties)) {
    for (const [key, child] of Object.entries(source.properties))
      if (isRecord(child) && 'const' in child) value[key] = child.const;
  }
  emit('update:modelValue', value);
}

function setProperty(key: string, value: unknown) {
  emitObject({ ...objectValue.value, [key]: value });
}

function addMapEntry() {
  let index = Object.keys(mapValue.value).length + 1;
  let key = `key${index}`;
  while (key in mapValue.value) key = `key${++index}`;
  emitObject({
    ...mapValue.value,
    [key]: defaultValue(additionalSchema.value ?? {}),
  });
}

function renameMapEntry(oldKey: string, newKey: string) {
  if (!newKey.trim() || (newKey !== oldKey && newKey in mapValue.value)) return;
  const next: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(mapValue.value))
    next[key === oldKey ? newKey : key] = value;
  emitObject(next);
}

function setMapEntry(key: string, value: unknown) {
  emitObject({ ...mapValue.value, [key]: value });
}

function removeMapEntry(key: string) {
  const next = { ...mapValue.value };
  delete next[key];
  emitObject(next);
}

const arrayValue = computed<unknown[]>(() =>
  Array.isArray(props.modelValue) ? props.modelValue : [],
);

const itemSchema = computed<Schema | null>(() =>
  isRecord(props.schema.items) ? props.schema.items : null,
);

function addArrayItem() {
  emit('update:modelValue', [...arrayValue.value, defaultValue(itemSchema.value ?? {})]);
}

function setArrayItem(index: number, value: unknown) {
  const next = [...arrayValue.value];
  next[index] = value;
  emit('update:modelValue', next);
}

function removeArrayItem(index: number) {
  const next = [...arrayValue.value];
  next.splice(index, 1);
  emit('update:modelValue', next);
}

function setPrimitive(value: string | boolean) {
  if (typeOf(props.schema) === 'boolean') emit('update:modelValue', value === true);
  else if (['number', 'integer'].includes(typeOf(props.schema))) {
    const number = Number(value);
    if (Number.isFinite(number))
      emit('update:modelValue', typeOf(props.schema) === 'integer' ? Math.trunc(number) : number);
  } else emit('update:modelValue', value);
}

function setEnumValue(value: string) {
  const match = enumValues(props.schema)?.find((option) => String(option) === value);
  emit('update:modelValue', match ?? value);
}

function stringValue(value: unknown): string {
  return typeof value === 'string'
    ? value
    : value === undefined || value === null
      ? ''
      : String(value);
}
</script>

<template>
  <div class="schema-value">
    <FormSelect
      v-if="variantOptions.length"
      :model-value="String(chosen)"
      :options="variantOptions"
      @change="chooseVariant"
    />

    <template v-if="selectedVariant">
      <ToolSchemaField
        :schema="selectedVariant"
        :model-value="modelValue"
        @update:model-value="emit('update:modelValue', $event)"
      />
    </template>

    <template v-else-if="isObjectSchema(schema)">
      <div v-if="properties.length" class="schema-subfields">
        <label v-for="[key, child] in properties" :key="key" class="schema-subfield">
          <span>
            {{ key }}
            <em v-if="required.has(key)">required</em>
          </span>
          <span v-if="'const' in child" class="schema-const">{{ child.const }}</span>
          <ToolSchemaField
            v-else
            :schema="child"
            :model-value="objectValue[key]"
            @update:model-value="setProperty(key, $event)"
          />
          <small v-if="typeof child.description === 'string'">{{ child.description }}</small>
        </label>
      </div>
      <div v-if="additionalSchema" class="schema-map">
        <div v-for="[key, value] in Object.entries(mapValue)" :key="key" class="schema-map-row">
          <input
            class="field-input schema-map-key"
            :value="key"
            @change="renameMapEntry(key, ($event.target as HTMLInputElement).value)"
          />
          <ToolSchemaField
            :schema="additionalSchema"
            :model-value="value"
            @update:model-value="setMapEntry(key, $event)"
          />
          <button
            type="button"
            class="schema-array-remove"
            title="Remove entry"
            @click="removeMapEntry(key)"
          >
            ×
          </button>
        </div>
        <button type="button" class="schema-array-add" @click="addMapEntry">Add entry</button>
      </div>
    </template>

    <template v-else-if="typeOf(schema) === 'array'">
      <div class="schema-array">
        <div v-if="!arrayValue.length" class="schema-array-empty">No items</div>
        <div v-for="(item, index) in arrayValue" :key="index" class="schema-array-item">
          <ToolSchemaField
            v-if="itemSchema"
            :schema="itemSchema"
            :model-value="item"
            @update:model-value="setArrayItem(index, $event)"
          />
          <input
            v-else
            class="field-input"
            :value="stringValue(item)"
            @input="setArrayItem(index, ($event.target as HTMLInputElement).value)"
          />
          <button
            type="button"
            class="schema-array-remove"
            title="Remove item"
            @click="removeArrayItem(index)"
          >
            ×
          </button>
        </div>
        <button type="button" class="schema-array-add" @click="addArrayItem">Add item</button>
      </div>
    </template>

    <FormSelect
      v-else-if="enumValues(schema)"
      :model-value="stringValue(modelValue)"
      :options="
        enumValues(schema)!.map((value) => ({
          value: String(value),
          label: String(value),
        }))
      "
      @change="setEnumValue"
    />

    <label v-else-if="typeOf(schema) === 'boolean'" class="schema-checkbox">
      <input
        type="checkbox"
        :checked="modelValue === true"
        @change="setPrimitive(($event.target as HTMLInputElement).checked)"
      />
      <span>{{ modelValue === true ? 'true' : 'false' }}</span>
    </label>

    <input
      v-else
      class="field-input"
      :type="['number', 'integer'].includes(typeOf(schema)) ? 'number' : 'text'"
      :value="stringValue(modelValue)"
      :placeholder="typeof schema.description === 'string' ? schema.description : ''"
      @input="setPrimitive(($event.target as HTMLInputElement).value)"
    />
  </div>
</template>
