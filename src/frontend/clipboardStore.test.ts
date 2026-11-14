import { describe, expect, it, vi } from 'vitest';

import { createClipboardStore } from './clipboardStore';
import type {
  ClipboardAdapter,
  ClipboardItem,
  ClipboardListParams,
  ClipboardListResult,
  ClipboardSettings,
  ClipboardSettingsState,
  Unlisten,
  WindowActivation,
} from './types';

const baseSettings: ClipboardSettingsState = {
  captureEnabled: true,
  retentionLimit: 500,
  shortcut: 'CommandOrControl+Alt+V',
  shortcutRegistered: true,
  shortcutRegisterError: null,
  unavailableReason: null,
  systemShortcutCommand: null,
};

function item(id: string): ClipboardItem {
  return {
    id,
    kind: 'text',
    preview: `preview ${id}`,
    contentText: `content ${id}`,
    contentTruncated: false,
    filePaths: [],
    previewDataUrl: null,
    isPinned: false,
    createdAt: '2026-11-15T10:00:00+08:00',
    lastCopiedAt: null,
  };
}

function fakeAdapter(overrides: Partial<ClipboardAdapter> = {}) {
  let historyChanged: (() => void) | undefined;
  let previewActivated: ((activation: WindowActivation) => void) | undefined;
  const adapter: ClipboardAdapter = {
    mode: 'standalone',
    connect: vi.fn(async () => undefined),
    list: vi.fn(async (): Promise<ClipboardListResult> => ({ items: [], nextCursor: null })),
    get: vi.fn(async (id: string) => item(id)),
    pin: vi.fn(async (_id: string, _isPinned: boolean) => undefined),
    delete: vi.fn(async (_id: string) => undefined),
    clearUnpinned: vi.fn(async () => 0),
    restore: vi.fn(async (_id: string) => undefined),
    getSettings: vi.fn(async () => baseSettings),
    updateSettings: vi.fn(async (settings: ClipboardSettings) => ({ ...baseSettings, ...settings })),
    onHistoryChanged: vi.fn(async (handler: () => void): Promise<Unlisten> => {
      historyChanged = handler;
      return () => undefined;
    }),
    onWindowActivation: vi.fn(async (_windowId, handler): Promise<Unlisten> => {
      previewActivated = handler;
      return () => undefined;
    }),
    getWindowActivation: vi.fn(async () => null),
    openWindow: vi.fn(async () => undefined),
    closeWindow: vi.fn(async () => undefined),
    ...overrides,
  };
  return {
    adapter,
    emitHistoryChanged: () => historyChanged?.(),
    activatePreview: (activation: WindowActivation) => previewActivated?.(activation),
  };
}

describe('clipboard store', () => {
  it('sends filters to the service and appends the next cursor page', async () => {
    const list = vi.fn(async (params: ClipboardListParams): Promise<ClipboardListResult> => {
      if (params.cursor === 'next-page') return { items: [item('third')], nextCursor: null };
      return { items: [item('first'), item('second')], nextCursor: 'next-page' };
    });
    const { adapter } = fakeAdapter({ list });
    const store = createClipboardStore(adapter);

    await store.startHistory();
    await store.setFilters('files', ' report ');
    await store.loadMore();

    expect(list).toHaveBeenNthCalledWith(2, {
      kind: 'files',
      query: ' report ',
      cursor: null,
      limit: 50,
    });
    expect(list).toHaveBeenNthCalledWith(3, {
      kind: 'files',
      query: ' report ',
      cursor: 'next-page',
      limit: 50,
    });
    expect(store.items.value.map(({ id }) => id)).toEqual(['first', 'second', 'third']);
    expect(store.nextCursor.value).toBeNull();
  });

  it('refreshes the current server-side filter when history changes', async () => {
    const list = vi
      .fn<ClipboardAdapter['list']>()
      .mockResolvedValueOnce({ items: [item('before')], nextCursor: null })
      .mockResolvedValueOnce({ items: [item('after')], nextCursor: null });
    const runtime = fakeAdapter({ list });
    const store = createClipboardStore(runtime.adapter);

    await store.startHistory();
    runtime.emitHistoryChanged();
    await vi.waitFor(() => expect(store.items.value[0]?.id).toBe('after'));

    expect(list).toHaveBeenLastCalledWith({ kind: null, query: '', cursor: null, limit: 50 });
  });

  it('restores the previous settings when the service rejects an update', async () => {
    const failure = {
      code: 'CLIPBOARD_SERVICE_SHORTCUT_REGISTER_FAILED',
      message: 'shortcut is already in use',
      retryable: false,
    };
    const { adapter } = fakeAdapter({
      updateSettings: vi.fn(async () => { throw failure; }),
    });
    const store = createClipboardStore(adapter);
    await store.startHistory({ includeSettings: true });
    const next: ClipboardSettings = {
      captureEnabled: false,
      retentionLimit: 120,
      shortcut: 'CommandOrControl+Shift+X',
    };

    await expect(store.saveSettings(next)).rejects.toBe(failure);

    expect(store.settings.value).toEqual(baseSettings);
    expect(store.settingsError.value).toBe('shortcut is already in use');
  });

  it('loads preview details by the latest activated item id', async () => {
    const runtime = fakeAdapter({
      getWindowActivation: vi.fn(async () => ({ id: 'initial' })),
    });
    const store = createClipboardStore(runtime.adapter);

    await store.startPreview();
    runtime.activatePreview({ id: 'latest' });
    await vi.waitFor(() => expect(store.previewItem.value?.id).toBe('latest'));

    expect(runtime.adapter.get).toHaveBeenNthCalledWith(1, 'initial');
    expect(runtime.adapter.get).toHaveBeenNthCalledWith(2, 'latest');
    expect(runtime.adapter.onWindowActivation).toHaveBeenCalledWith('preview', expect.any(Function));
  });

  it('does not let a stale initial activation replace a newer preview event', async () => {
    let resolveInitial: ((activation: WindowActivation) => void) | undefined;
    const runtime = fakeAdapter({
      getWindowActivation: vi.fn(async () => new Promise<WindowActivation>((resolve) => {
        resolveInitial = resolve;
      })),
    });
    const store = createClipboardStore(runtime.adapter);

    const starting = store.startPreview();
    await vi.waitFor(() => expect(resolveInitial).toBeTypeOf('function'));
    runtime.activatePreview({ id: 'latest' });
    resolveInitial?.({ id: 'stale' });
    await starting;
    await vi.waitFor(() => expect(store.previewItem.value?.id).toBe('latest'));

    expect(runtime.adapter.get).toHaveBeenCalledTimes(1);
    expect(runtime.adapter.get).toHaveBeenCalledWith('latest');
  });

  it('reconnects when retrying after the initial service connection failed', async () => {
    const connect = vi
      .fn<ClipboardAdapter['connect']>()
      .mockRejectedValueOnce(new Error('service is starting'))
      .mockResolvedValueOnce(undefined);
    const runtime = fakeAdapter({ connect });
    const store = createClipboardStore(runtime.adapter);

    await store.startHistory();
    expect(store.error.value).toBe('service is starting');

    await store.retryHistory();

    expect(connect).toHaveBeenCalledTimes(2);
    expect(runtime.adapter.list).toHaveBeenCalledOnce();
    expect(store.error.value).toBeNull();
  });

  it('reconnects when retrying after an established service connection was lost', async () => {
    const connect = vi.fn<ClipboardAdapter['connect']>(async () => undefined);
    const list = vi
      .fn<ClipboardAdapter['list']>()
      .mockResolvedValueOnce({ items: [item('before')], nextCursor: null })
      .mockRejectedValueOnce(new Error('service disconnected'))
      .mockResolvedValueOnce({ items: [item('after')], nextCursor: null });
    const runtime = fakeAdapter({ connect, list });
    const store = createClipboardStore(runtime.adapter);
    await store.startHistory();
    await store.refresh();
    expect(store.error.value).toBe('service disconnected');

    await store.retryHistory();

    expect(connect).toHaveBeenCalledTimes(2);
    expect(store.items.value.map(({ id }) => id)).toEqual(['after']);
    expect(store.error.value).toBeNull();
  });

  it('does not append an obsolete cursor page after filters changed', async () => {
    let resolveOldPage: ((result: ClipboardListResult) => void) | undefined;
    const list = vi.fn(async (params: ClipboardListParams): Promise<ClipboardListResult> => {
      if (params.cursor) {
        return new Promise((resolve) => { resolveOldPage = resolve; });
      }
      if (params.kind === 'image') return { items: [{ ...item('image-only'), kind: 'image' }], nextCursor: null };
      return { items: [item('first')], nextCursor: 'old-page' };
    });
    const runtime = fakeAdapter({ list });
    const store = createClipboardStore(runtime.adapter);
    await store.startHistory();

    const loadingOldPage = store.loadMore();
    await vi.waitFor(() => expect(resolveOldPage).toBeTypeOf('function'));
    await store.setFilters('image', 'shot');
    resolveOldPage?.({ items: [item('obsolete-text')], nextCursor: null });
    await loadingOldPage;

    expect(store.items.value.map(({ id }) => id)).toEqual(['image-only']);
    expect(store.nextCursor.value).toBeNull();
  });
});
