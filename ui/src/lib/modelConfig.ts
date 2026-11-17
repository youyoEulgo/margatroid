export const MODEL_PROVIDERS = ['openai', 'anthropic', 'deepseek', 'openrouter'] as const;
export type ModelProvider = (typeof MODEL_PROVIDERS)[number];

export const MODEL_API_TYPES = ['chat_completions', 'messages', 'responses'] as const;
export type ModelApiType = (typeof MODEL_API_TYPES)[number];

export interface ModelRoute {
  id: string;
  model: string;
  provider: string;
  apiType: string;
  baseUrl: string;
  apiKey: string;
  contextWindow: string;
  thinking: string;
  reasoningEffort: string;
}

export interface ModelRouteOption {
  id: string;
  model: string | null;
}

const API_TYPES_BY_PROVIDER: Record<string, readonly string[]> = {
  openai: ['chat_completions', 'responses'],
  anthropic: ['messages'],
  deepseek: ['chat_completions', 'messages', 'responses'],
  openrouter: ['chat_completions'],
};

export function supportedApiTypes(provider: string): readonly string[] {
  return API_TYPES_BY_PROVIDER[provider] ?? MODEL_API_TYPES;
}

export function providerSupports(provider: string, apiType: string): boolean {
  return supportedApiTypes(provider).includes(apiType);
}

function stripComment(value: string): string {
  let quote: string | null = null;
  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    if (quote) {
      if (char === '\\' && quote === '"') index += 1;
      else if (char === quote) quote = null;
    } else if (char === '"' || char === "'") quote = char;
    else if (char === '#') return value.slice(0, index);
  }
  return value;
}

function tomlValue(value: string): string | null {
  const trimmed = stripComment(value).trim();
  if (!trimmed) return null;
  if (trimmed.startsWith('"') && trimmed.endsWith('"')) {
    try {
      return JSON.parse(trimmed) as string;
    } catch {
      return trimmed.slice(1, -1);
    }
  }
  if (trimmed.startsWith("'") && trimmed.endsWith("'")) return trimmed.slice(1, -1);
  return trimmed;
}

function tomlString(value: string): string {
  return JSON.stringify(value);
}

export function parseModelConfig(content: string | null): ModelRoute[] {
  if (!content) return [];
  const routes: ModelRoute[] = [];
  for (const section of content.split(/^\s*\[\[models\]\]\s*$/mu).slice(1)) {
    const fields = new Map<string, string>();
    for (const line of section.split('\n')) {
      const match = /^\s*([A-Za-z0-9_.-]+)\s*=\s*(.+?)\s*$/u.exec(line);
      if (!match) continue;
      const value = match[2];
      if (!value) continue;
      const parsed = tomlValue(value);
      if (parsed !== null) fields.set(match[1]!, parsed);
    }
    routes.push({
      id: fields.get('id') ?? '',
      model: fields.get('model') ?? '',
      provider: fields.get('provider') ?? 'openai',
      apiType: fields.get('api_type') ?? 'chat_completions',
      baseUrl: fields.get('base_url') ?? '',
      apiKey: fields.get('api_key') ?? '',
      contextWindow: fields.get('context_window') ?? '1m',
      thinking: fields.get('thinking') ?? '',
      reasoningEffort: fields.get('reasoning_effort') ?? '',
    });
  }
  return routes;
}

export function parseModelRoutes(content: string | null): ModelRouteOption[] {
  return parseModelConfig(content)
    .filter((route) => route.id)
    .map((route) => ({ id: route.id, model: route.model || null }));
}

export function serializeModelConfig(routes: ModelRoute[]): string {
  return routes
    .map((route) => {
      const lines = [
        '[[models]]',
        `id = ${tomlString(route.id)}`,
        `model = ${tomlString(route.model)}`,
        `provider = ${tomlString(route.provider)}`,
        `api_type = ${tomlString(route.apiType)}`,
        `base_url = ${tomlString(route.baseUrl)}`,
        `api_key = ${tomlString(route.apiKey)}`,
        `context_window = ${tomlString(route.contextWindow || '1m')}`,
      ];
      if (route.thinking) lines.push(`thinking = ${tomlString(route.thinking)}`);
      if (route.reasoningEffort)
        lines.push(`reasoning_effort = ${tomlString(route.reasoningEffort)}`);
      return `${lines.join('\n')}\n`;
    })
    .join('\n');
}

function isControl(value: string): boolean {
  return [...value].some((char) => {
    const code = char.codePointAt(0) ?? 0;
    return code < 0x20 || code === 0x7f;
  });
}

function parseContextWindow(value: string): number | null {
  const match = /^([1-9]\d*)([kmbt])$/iu.exec(value.trim());
  if (!match) return null;
  const multiplier = { k: 1_000, m: 1_000_000, b: 1_000_000_000, t: 1_000_000_000_000 }[
    match[2]!.toLowerCase() as 'k' | 'm' | 'b' | 't'
  ];
  const parsed = Number(match[1]);
  if (!Number.isSafeInteger(parsed)) return null;
  const total = parsed * multiplier;
  return total > 0 && total <= Number.MAX_SAFE_INTEGER ? total : null;
}

export function validateModelRoute(route: ModelRoute): string | null {
  if (!route.id.trim() || route.id.length > 256 || isControl(route.id))
    return 'Route id must be 1–256 characters and contain no control characters.';
  if (!route.model.trim()) return 'Model name is required.';
  if (!MODEL_PROVIDERS.includes(route.provider as ModelProvider)) return 'Invalid provider.';
  if (!MODEL_API_TYPES.includes(route.apiType as ModelApiType)) return 'Invalid API type.';
  if (!providerSupports(route.provider, route.apiType))
    return `${route.provider} does not support ${route.apiType}.`;
  let url: URL;
  try {
    url = new URL(route.baseUrl);
  } catch {
    return 'Base URL must be a valid HTTP URL.';
  }
  if (
    !['http:', 'https:'].includes(url.protocol) ||
    !url.hostname ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    return 'Base URL must be an HTTP API prefix without credentials, query or fragment.';
  if (!route.apiKey.trim() || isControl(route.apiKey))
    return 'API key is required and must not contain control characters.';
  if (parseContextWindow(route.contextWindow) === null)
    return 'Context window must be an integer followed by k, m, b or t.';
  const advanced = route.thinking !== '' || route.reasoningEffort !== '';
  if (
    advanced &&
    !(['deepseek', 'openrouter'].includes(route.provider) && route.apiType === 'chat_completions')
  )
    return 'Thinking and reasoning effort are supported only for DeepSeek/OpenRouter chat_completions.';
  if (route.thinking && !['enabled', 'disabled'].includes(route.thinking))
    return 'Thinking must be enabled or disabled.';
  if (route.thinking === 'enabled') {
    if (route.reasoningEffort !== 'high' && route.reasoningEffort !== 'max')
      return 'Enabled thinking requires reasoning effort high or max.';
    if (route.reasoningEffort === 'max' && route.provider !== 'deepseek')
      return 'Reasoning effort max is supported only by DeepSeek.';
  } else if (route.thinking === 'disabled' && route.reasoningEffort) {
    return 'Disabled thinking cannot set reasoning effort.';
  } else if (!route.thinking && route.reasoningEffort) {
    return 'Reasoning effort requires enabled thinking.';
  }
  return null;
}
