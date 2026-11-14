import { emit, listen } from '@tauri-apps/api/event';
import { ref } from 'vue';

import type { ThemeName, Unlisten } from './types';

export type { ThemeName } from './types';

const STORAGE_KEY = 'atclipboard.theme';
const THEME_EVENT = 'clipboard://theme-changed';

export interface ThemeBridge {
  read(): ThemeName;
  write(theme: ThemeName): void;
  apply(theme: ThemeName): void;
  broadcast(theme: ThemeName): Promise<void>;
  listen(handler: (theme: ThemeName) => void): Promise<Unlisten>;
}

const browserBridge: ThemeBridge = {
  read() {
    return localStorage.getItem(STORAGE_KEY) === 'light' ? 'light' : 'dark';
  },
  write(theme) {
    localStorage.setItem(STORAGE_KEY, theme);
  },
  apply(theme) {
    document.documentElement.dataset.theme = theme;
  },
  broadcast(theme) {
    return emit(THEME_EVENT, theme);
  },
  async listen(handler) {
    return listen<ThemeName>(THEME_EVENT, ({ payload }) => {
      if (payload === 'dark' || payload === 'light') handler(payload);
    });
  },
};

export function createThemeController(bridge: ThemeBridge = browserBridge) {
  const theme = ref<ThemeName>(bridge.read());
  let unlisten: Unlisten | null = null;

  async function start() {
    bridge.apply(theme.value);
    unlisten = await bridge.listen((next) => {
      theme.value = next;
      bridge.write(next);
      bridge.apply(next);
    });
  }

  async function toggle() {
    const next = theme.value === 'dark' ? 'light' : 'dark';
    theme.value = next;
    bridge.write(next);
    bridge.apply(next);
    await bridge.broadcast(next);
  }

  function destroy() {
    unlisten?.();
    unlisten = null;
  }

  return { theme, start, toggle, destroy };
}
