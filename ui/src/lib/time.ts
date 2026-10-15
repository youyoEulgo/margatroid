export function formatRecordTime(timestamp: number, seconds = false): string {
  if (
    !Number.isFinite(timestamp) ||
    timestamp <= 0 ||
    !Number.isFinite(new Date(timestamp).getTime())
  )
    return 'Time unavailable';
  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    ...(seconds ? { second: '2-digit' as const } : {}),
  }).format(timestamp);
}
