import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import type {
  ClipboardAdapter,
  ClipboardItem,
  ClipboardListParams,
  ClipboardListResult,
  ClipboardSettings,
  ClipboardSettingsState,
  ClipboardWindowId,
  RuntimePort,
  WindowActivation,
} from './types';

const tauriPort: RuntimePort = {
  invoke(command, args) {
    return invoke(command, args);
  },
  async listen(event, handler) {
    return listen(event, ({ payload }) => handler(payload));
  },
};

function request<T>(port: RuntimePort, method: string, params: Record<string, unknown>): Promise<T> {
  return port.invoke('clipboard_service_request', { method, params }) as Promise<T>;
}

export function createStandaloneAdapter(port: RuntimePort = tauriPort): ClipboardAdapter {
  return {
    mode: 'standalone',
    async connect() {
      await port.invoke('connect_clipboard_service');
    },
    list(params: ClipboardListParams) {
      const requestParams: Record<string, unknown> = {
        query: params.query,
        limit: params.limit,
      };
      if (params.kind) requestParams.kind = params.kind;
      if (params.cursor) requestParams.cursor = params.cursor;
      return request<ClipboardListResult>(port, 'clipboard.history.list', requestParams);
    },
    get(id: string) {
      return request<ClipboardItem>(port, 'clipboard.history.get', { id });
    },
    async pin(id: string, isPinned: boolean) {
      await request(port, 'clipboard.history.pin', { id, isPinned });
    },
    async delete(id: string) {
      await request(port, 'clipboard.history.delete', { id });
    },
    async clearUnpinned() {
      const result = await request<{ deletedCount: number }>(
        port,
        'clipboard.history.clearUnpinned',
        {},
      );
      return result.deletedCount;
    },
    async restore(id: string) {
      await request(port, 'clipboard.history.restore', { id });
    },
    getSettings() {
      return request<ClipboardSettingsState>(port, 'clipboard.settings.get', {});
    },
    updateSettings(settings: ClipboardSettings) {
      return request<ClipboardSettingsState>(port, 'clipboard.settings.update', { settings });
    },
    onHistoryChanged(handler: () => void) {
      return port.listen('clipboard://history-changed', handler);
    },
    onWindowActivation(windowId, handler) {
      return port.listen(`clipboard://${windowId}-activation`, (payload) => {
        if (isWindowActivation(payload)) handler(payload);
      });
    },
    async getWindowActivation(windowId) {
      const activation = await port.invoke('get_clipboard_window_activation', { windowId });
      return isWindowActivation(activation) ? activation : null;
    },
    async openWindow(windowId: ClipboardWindowId, activation?: WindowActivation) {
      await port.invoke('open_clipboard_window', {
        windowId,
        payload: activation ?? {},
      });
    },
    async closeWindow(windowId: ClipboardWindowId) {
      await port.invoke('close_clipboard_window', { windowId });
    },
  };
}

function isWindowActivation(value: unknown): value is WindowActivation {
  return typeof value === 'object'
    && value !== null
    && typeof (value as { id?: unknown }).id === 'string'
    && (value as { id: string }).id.length > 0;
}
