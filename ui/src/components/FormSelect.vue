<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref } from 'vue';
import { Check, ChevronDown } from 'lucide-vue-next';

export interface FormSelectOption {
  value: string;
  label: string;
}

const props = defineProps<{
  modelValue: string;
  options: readonly (string | FormSelectOption)[];
  placeholder?: string;
  disabled?: boolean;
}>();
const emit = defineEmits<{
  'update:modelValue': [value: string];
  change: [value: string];
}>();

const trigger = ref<HTMLElement | null>(null);
const menu = ref<HTMLElement | null>(null);
const open = ref(false);
const position = ref({ left: 0, top: 0, width: 0, maxHeight: 220 });

const normalizedOptions = computed(() =>
  props.options.map((option) =>
    typeof option === 'string' ? { value: option, label: option } : option,
  ),
);
const label = computed(() => {
  const selected = normalizedOptions.value.find((option) => option.value === props.modelValue);
  return selected?.label ?? props.placeholder ?? 'default';
});

function place() {
  const rect = trigger.value?.getBoundingClientRect();
  if (!rect) return;
  const width = Math.max(rect.width, 180);
  const left = Math.min(Math.max(8, rect.left), window.innerWidth - width - 8);
  const below = window.innerHeight - rect.bottom - 8;
  const above = rect.top - 8;
  const openUp = below < 180 && above > below;
  const maxHeight = Math.min(240, Math.max(120, openUp ? above : below));
  position.value = {
    left,
    top: openUp ? rect.top - Math.min(maxHeight, 240) - 4 : rect.bottom + 4,
    width,
    maxHeight,
  };
}

async function toggle() {
  if (props.disabled) return;
  if (open.value) {
    open.value = false;
    return;
  }
  open.value = true;
  await nextTick();
  place();
  document.addEventListener('pointerdown', outside, true);
  window.addEventListener('resize', close);
  window.addEventListener('scroll', scrolled, true);
}

function close() {
  open.value = false;
  document.removeEventListener('pointerdown', outside, true);
  window.removeEventListener('resize', close);
  window.removeEventListener('scroll', scrolled, true);
}

function scrolled(event: Event) {
  const target = event.target;
  if (target instanceof Node && (menu.value?.contains(target) || trigger.value?.contains(target)))
    return;
  close();
}

function outside(event: PointerEvent) {
  const target = event.target;
  if (!(target instanceof Node)) return;
  if (trigger.value?.contains(target) || menu.value?.contains(target)) return;
  close();
}

function choose(value: string) {
  emit('update:modelValue', value);
  emit('change', value);
  close();
}

onBeforeUnmount(close);
</script>

<template>
  <div ref="trigger" class="form-select">
    <button
      type="button"
      class="form-select-trigger"
      :disabled="disabled"
      :aria-expanded="open"
      aria-haspopup="listbox"
      @click="toggle"
    >
      <span>{{ label }}</span>
      <ChevronDown :class="['form-select-caret', { 'is-open': open }]" :size="13" />
    </button>
    <Teleport to="body">
      <div
        v-if="open"
        ref="menu"
        class="form-select-menu"
        role="listbox"
        @wheel.stop
        @scroll.stop
        :style="{
          left: `${position.left}px`,
          top: `${position.top}px`,
          width: `${position.width}px`,
          maxHeight: `${position.maxHeight}px`,
        }"
      >
        <button
          v-for="option in normalizedOptions"
          :key="option.value"
          type="button"
          :class="['form-select-option', { active: option.value === modelValue }]"
          role="option"
          :aria-selected="option.value === modelValue"
          @click="choose(option.value)"
        >
          <span>{{ option.label || placeholder || 'default' }}</span>
          <Check v-if="option.value === modelValue" :size="13" />
        </button>
      </div>
    </Teleport>
  </div>
</template>
