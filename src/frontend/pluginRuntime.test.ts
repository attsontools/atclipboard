import { describe, expect, it, vi } from 'vitest';

import { createPluginRuntime } from './pluginRuntime';

describe('clipboard plugin runtime', () => {
  it('routes clipboard, auxiliary windows, events and theme through Host API 2', async () => {
    const notifications = new Map<string, (params: unknown) => void>();
    let themeChanged: ((theme: 'dark' | 'light') => void) | undefined;
    const call = vi.fn(async (method: string, params: unknown) => {
      if (method !== 'host.clipboard.history') return { opened: true };
      const request = params as { method: string };
      if (request.method === 'clipboard.history.list') {
        return { items: [], nextCursor: null };
      }
      if (request.method === 'clipboard.settings.get') {
        return {
          captureEnabled: true,
          retentionLimit: 500,
          shortcut: 'CommandOrControl+Alt+V',
          shortcutRegistered: false,
          shortcutRegisterError: 'Wayland does not expose global shortcuts',
          unavailableReason: 'Use a desktop custom shortcut',
          systemShortcutCommand: 'atclipboard --history',
        };
      }
      return {};
    });
    const runtime = createPluginRuntime(async (appId) => {
      expect(appId).toBe('com.attson.clipboard');
      return {
        handshake: async () => ({
          theme: 'light' as const,
          capabilities: [
            'host.theme.read',
            'host.ui.auxiliary',
            'host.clipboard.history',
          ],
        }),
        call,
        onThemeChange(listener) {
          themeChanged = listener;
          return () => undefined;
        },
        onNotification(method, listener) {
          notifications.set(method, listener);
          return () => notifications.delete(method);
        },
        close: vi.fn(),
      };
    });
    const historyChanged = vi.fn();
    const activated = vi.fn();
    const themes: string[] = [];

    await runtime.adapter.connect();
    await runtime.adapter.onHistoryChanged(historyChanged);
    await runtime.adapter.onWindowActivation('preview', activated);
    await runtime.themeBridge.listen((theme) => themes.push(theme));
    await runtime.adapter.list({ kind: 'image', query: 'shot', cursor: null, limit: 50 });
    const settings = await runtime.adapter.getSettings();
    await runtime.adapter.openWindow('preview', { id: 'clip-7' });
    await runtime.adapter.closeWindow('preview');
    notifications.get('host.clipboard.historyChanged')?.({
      type: 'clipboard.history.changed',
      revision: 4,
    });
    notifications.get('host.ui.activated')?.({
      windowId: 'preview',
      payload: { id: 'clip-8' },
    });
    themeChanged?.('dark');

    expect(call).toHaveBeenCalledWith('host.clipboard.history', {
      method: 'clipboard.history.list',
      params: { kind: 'image', query: 'shot', limit: 50 },
    });
    expect(call).toHaveBeenCalledWith('host.clipboard.history', {
      method: 'clipboard.settings.get',
      params: {},
    });
    expect(settings.systemShortcutCommand).toBe(
      'attool --plugin-window com.attson.clipboard/history',
    );
    expect(call).toHaveBeenCalledWith('host.ui.open', {
      windowId: 'preview',
      payload: { id: 'clip-7' },
    });
    expect(call).toHaveBeenCalledWith('host.ui.close', {});
    expect(historyChanged).toHaveBeenCalledOnce();
    expect(activated).toHaveBeenCalledWith({ id: 'clip-8' });
    expect(await runtime.adapter.getWindowActivation('preview')).toEqual({ id: 'clip-8' });
    expect(themes).toEqual(['light', 'dark']);
    expect(runtime.themeBridge.mutable).toBe(false);
  });

  it('rejects a host missing any required clipboard capability', async () => {
    const close = vi.fn();
    const runtime = createPluginRuntime(async () => ({
      handshake: async () => ({
        theme: 'dark',
        capabilities: ['host.theme.read', 'host.clipboard.history'],
      }),
      call: async () => ({}),
      onThemeChange: () => () => undefined,
      onNotification: () => () => undefined,
      close,
    }));

    await expect(runtime.adapter.connect())
      .rejects.toThrow('HOST_PERMISSION_DENIED: host.ui.auxiliary');
    expect(close).toHaveBeenCalledOnce();
  });

  it('does not overwrite a newer theme event with the handshake snapshot', async () => {
    let themeChanged: ((theme: 'dark' | 'light') => void) | undefined;
    const runtime = createPluginRuntime(async () => ({
      async handshake() {
        themeChanged?.('light');
        return {
          theme: 'dark',
          capabilities: [
            'host.theme.read',
            'host.ui.auxiliary',
            'host.clipboard.history',
          ],
        };
      },
      call: async () => ({}),
      onThemeChange(listener) {
        themeChanged = listener;
        return () => undefined;
      },
      onNotification: () => () => undefined,
      close: () => undefined,
    }));
    const themes: string[] = [];

    await runtime.themeBridge.listen((theme) => themes.push(theme));

    expect(themes).toEqual(['light']);
  });
});
