import type { ThemeBridge } from './theme';
import type {
  ClipboardAdapter,
  ClipboardItem,
  ClipboardListParams,
  ClipboardListResult,
  ClipboardSettings,
  ClipboardSettingsState,
  ClipboardWindowId,
  ThemeName,
  Unlisten,
  WindowActivation,
} from './types';

const APP_ID = 'com.attson.clipboard';
const REQUIRED_CAPABILITIES = [
  'host.theme.read',
  'host.ui.auxiliary',
  'host.clipboard.history',
] as const;
const PLUGIN_HISTORY_COMMAND = 'attool --plugin-window com.attson.clipboard/history';

interface PluginHandshake {
  theme: ThemeName;
  capabilities: string[];
}

export interface PluginConnection {
  handshake(): Promise<PluginHandshake>;
  call(method: string, params?: unknown): Promise<unknown>;
  onThemeChange(listener: (theme: ThemeName) => void): Unlisten;
  onNotification(method: string, listener: (params: unknown) => void): Unlisten;
  close(): void;
}

type ConnectPlugin = (appId: string) => Promise<PluginConnection>;

export function createPluginRuntime(connectPlugin: ConnectPlugin): {
  adapter: ClipboardAdapter;
  themeBridge: ThemeBridge;
} {
  let connection: PluginConnection | null = null;
  let connecting: Promise<void> | null = null;
  let theme: ThemeName = 'dark';
  let themeRevision = 0;
  let previewActivation: WindowActivation | null = null;
  const historyListeners = new Set<() => void>();
  const activationListeners = new Set<(activation: WindowActivation) => void>();
  const themeListeners = new Set<(theme: ThemeName) => void>();

  async function connect() {
    if (connection) return;
    if (connecting) return connecting;
    connecting = (async () => {
      const candidate = await connectPlugin(APP_ID);
      const handshakeThemeRevision = themeRevision;
      const removeHistory = candidate.onNotification('host.clipboard.historyChanged', (value) => {
        if (!isHistoryChanged(value)) return;
        for (const listener of historyListeners) listener();
      });
      const removeActivation = candidate.onNotification('host.ui.activated', (value) => {
        const activation = readActivation(value);
        if (!activation) return;
        previewActivation = activation;
        for (const listener of activationListeners) listener(activation);
      });
      const removeTheme = candidate.onThemeChange((next) => {
        themeRevision += 1;
        theme = next;
        for (const listener of themeListeners) listener(next);
      });
      try {
        const handshake = await candidate.handshake();
        for (const capability of REQUIRED_CAPABILITIES) {
          if (!handshake.capabilities.includes(capability)) {
            throw new Error(`HOST_PERMISSION_DENIED: ${capability}`);
          }
        }
        if (themeRevision === handshakeThemeRevision) theme = handshake.theme;
        connection = candidate;
      } catch (error) {
        removeHistory();
        removeActivation();
        removeTheme();
        candidate.close();
        throw error;
      }
    })().finally(() => {
      connecting = null;
    });
    return connecting;
  }

  async function request<T>(method: string, params: Record<string, unknown>): Promise<T> {
    await connect();
    return connection?.call('host.clipboard.history', { method, params }) as Promise<T>;
  }

  const adapter: ClipboardAdapter = {
    mode: 'plugin',
    connect,
    list(params: ClipboardListParams) {
      const requestParams: Record<string, unknown> = {
        query: params.query,
        limit: params.limit,
      };
      if (params.kind) requestParams.kind = params.kind;
      if (params.cursor) requestParams.cursor = params.cursor;
      return request<ClipboardListResult>('clipboard.history.list', requestParams);
    },
    get(id: string) {
      return request<ClipboardItem>('clipboard.history.get', { id });
    },
    async pin(id: string, isPinned: boolean) {
      await request('clipboard.history.pin', { id, isPinned });
    },
    async delete(id: string) {
      await request('clipboard.history.delete', { id });
    },
    async clearUnpinned() {
      const result = await request<{ deletedCount: number }>('clipboard.history.clearUnpinned', {});
      return result.deletedCount;
    },
    async restore(id: string) {
      await request('clipboard.history.restore', { id });
    },
    getSettings() {
      return request<ClipboardSettingsState>('clipboard.settings.get', {})
        .then(withPluginShortcutCommand);
    },
    updateSettings(settings: ClipboardSettings) {
      return request<ClipboardSettingsState>('clipboard.settings.update', { settings })
        .then(withPluginShortcutCommand);
    },
    async onHistoryChanged(listener) {
      await connect();
      historyListeners.add(listener);
      return () => historyListeners.delete(listener);
    },
    async onWindowActivation(windowId, listener) {
      await connect();
      if (windowId !== 'preview') return () => undefined;
      activationListeners.add(listener);
      return () => activationListeners.delete(listener);
    },
    async getWindowActivation(windowId) {
      await connect();
      return windowId === 'preview' ? previewActivation : null;
    },
    async openWindow(windowId: ClipboardWindowId, activation?: WindowActivation) {
      await connect();
      await connection?.call('host.ui.open', {
        windowId,
        payload: activation ?? {},
      });
    },
    async closeWindow() {
      await connect();
      await connection?.call('host.ui.close', {});
    },
  };

  const themeBridge: ThemeBridge = {
    mutable: false,
    read: () => theme,
    write: () => undefined,
    apply(next) {
      document.documentElement.dataset.theme = next;
    },
    broadcast: async () => undefined,
    async listen(listener) {
      await connect();
      themeListeners.add(listener);
      listener(theme);
      return () => themeListeners.delete(listener);
    },
  };

  return { adapter, themeBridge };
}

function withPluginShortcutCommand(settings: ClipboardSettingsState): ClipboardSettingsState {
  if (!settings.systemShortcutCommand) return settings;
  return { ...settings, systemShortcutCommand: PLUGIN_HISTORY_COMMAND };
}

function isHistoryChanged(value: unknown) {
  if (!isRecord(value)) return false;
  return value.type === 'clipboard.history.changed'
    && Number.isSafeInteger(value.revision);
}

function readActivation(value: unknown): WindowActivation | null {
  if (!isRecord(value) || value.windowId !== 'preview' || !isRecord(value.payload)) return null;
  const id = value.payload.id;
  return typeof id === 'string' && id.length > 0 ? { id } : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
