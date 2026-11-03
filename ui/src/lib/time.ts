const formatters = new Map<boolean, Intl.DateTimeFormat>();

function formatter(seconds: boolean): Intl.DateTimeFormat {
  let value = formatters.get(seconds);
  if (!value) {
    value = new Intl.DateTimeFormat(undefined, {
      hour: '2-digit',
      minute: '2-digit',
      ...(seconds ? { second: '2-digit' as const } : {}),
    });
    formatters.set(seconds, value);
  }
  return value;
}

export function formatRecordTime(timestamp: number, seconds = false): string {
  if (
    !Number.isFinite(timestamp) ||
    timestamp <= 0 ||
    !Number.isFinite(new Date(timestamp).getTime())
  )
    return 'Time unavailable';
  return formatter(seconds).format(timestamp);
}
