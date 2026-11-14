import { ref } from 'vue';

import type { ThemeName, Unlisten } from './types';

export type { ThemeName } from './types';

export interface ThemeBridge {
  mutable: boolean;
  read(): ThemeName;
  write(theme: ThemeName): void;
  apply(theme: ThemeName): void;
  broadcast(theme: ThemeName): Promise<void>;
  listen(handler: (theme: ThemeName) => void): Promise<Unlisten>;
}

export function createThemeController(bridge: ThemeBridge) {
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
    if (!bridge.mutable) return;
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

  return { theme, canToggle: bridge.mutable, start, toggle, destroy };
}
