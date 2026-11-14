import { emit, listen } from '@tauri-apps/api/event';

import type { ThemeBridge } from './theme';
import type { ThemeName } from './types';

const STORAGE_KEY = 'atclipboard.theme';
const THEME_EVENT = 'clipboard://theme-changed';

export const standaloneThemeBridge: ThemeBridge = {
  mutable: true,
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
