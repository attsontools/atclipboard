export function messageFromUnknown(reason: unknown) {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === 'string') return reason;
  if (typeof reason === 'object' && reason !== null) {
    const message = (reason as { message?: unknown }).message;
    if (typeof message === 'string' && message.length > 0) return message;
  }
  return String(reason);
}
