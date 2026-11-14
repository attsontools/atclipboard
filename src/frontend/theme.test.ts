import { describe, expect, it, vi } from 'vitest';

import { createThemeController } from './theme';
import type { ThemeBridge, ThemeName } from './theme';

describe('theme controller', () => {
  it('restores, persists and synchronizes the theme across windows', async () => {
    let listener: ((theme: ThemeName) => void) | undefined;
    const bridge: ThemeBridge = {
      read: vi.fn(() => 'light' as const),
      write: vi.fn(),
      apply: vi.fn(),
      broadcast: vi.fn(async () => undefined),
      listen: vi.fn(async (handler) => {
        listener = handler;
        return () => undefined;
      }),
    };
    const controller = createThemeController(bridge);

    await controller.start();
    await controller.toggle();
    listener?.('light');

    expect(bridge.apply).toHaveBeenNthCalledWith(1, 'light');
    expect(bridge.write).toHaveBeenCalledWith('dark');
    expect(bridge.broadcast).toHaveBeenCalledWith('dark');
    expect(bridge.apply).toHaveBeenLastCalledWith('light');
    expect(controller.theme.value).toBe('light');
  });
});
