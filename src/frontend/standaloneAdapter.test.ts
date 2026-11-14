import { describe, expect, it, vi } from 'vitest';

import { createStandaloneAdapter } from './standaloneAdapter';
import type { RuntimePort } from './types';

describe('standalone clipboard adapter', () => {
  it('maps bounded history operations to the service transport', async () => {
    const invoke = vi.fn(async (command: string) => {
      if (command === 'clipboard_service_request') {
        return { items: [], nextCursor: null };
      }
      return undefined;
    });
    const port: RuntimePort = {
      invoke,
      listen: vi.fn(async () => () => undefined),
    };
    const adapter = createStandaloneAdapter(port);

    await adapter.list({ kind: 'image', query: 'shot', cursor: null, limit: 50 });
    await adapter.openWindow('preview', { id: 'clip-7' });

    expect(invoke).toHaveBeenNthCalledWith(1, 'clipboard_service_request', {
      method: 'clipboard.history.list',
      params: { kind: 'image', query: 'shot', limit: 50 },
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'open_clipboard_window', {
      windowId: 'preview',
      payload: { id: 'clip-7' },
    });
  });
});
