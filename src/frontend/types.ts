export type ClipboardItemKind = 'text' | 'image' | 'files';
export type ClipboardFilterKind = ClipboardItemKind | 'all';
export type ClipboardWindowId = 'history' | 'preview';
export type ThemeName = 'dark' | 'light';
export type Unlisten = () => void;

export interface ClipboardItem {
  id: string;
  kind: ClipboardItemKind;
  preview: string;
  contentText: string;
  contentTruncated: boolean;
  filePaths: string[];
  previewDataUrl: string | null;
  isPinned: boolean;
  createdAt: string;
  lastCopiedAt: string | null;
}

export interface ClipboardListParams {
  kind: ClipboardItemKind | null;
  query: string;
  cursor: string | null;
  limit: number;
}

export interface ClipboardListResult {
  items: ClipboardItem[];
  nextCursor: string | null;
}

export interface ClipboardSettings {
  captureEnabled: boolean;
  retentionLimit: number;
  shortcut: string;
}

export interface ClipboardSettingsState extends ClipboardSettings {
  shortcutRegistered: boolean;
  shortcutRegisterError: string | null;
  unavailableReason: string | null;
  systemShortcutCommand: string | null;
}

export interface WindowActivation {
  id: string;
}

export interface ClipboardAdapter {
  mode: 'standalone' | 'plugin';
  connect(): Promise<void>;
  list(params: ClipboardListParams): Promise<ClipboardListResult>;
  get(id: string): Promise<ClipboardItem>;
  pin(id: string, isPinned: boolean): Promise<void>;
  delete(id: string): Promise<void>;
  clearUnpinned(): Promise<number>;
  restore(id: string): Promise<void>;
  getSettings(): Promise<ClipboardSettingsState>;
  updateSettings(settings: ClipboardSettings): Promise<ClipboardSettingsState>;
  onHistoryChanged(handler: () => void): Promise<Unlisten>;
  onWindowActivation(
    windowId: ClipboardWindowId,
    handler: (activation: WindowActivation) => void,
  ): Promise<Unlisten>;
  getWindowActivation(windowId: ClipboardWindowId): Promise<WindowActivation | null>;
  openWindow(windowId: ClipboardWindowId, activation?: WindowActivation): Promise<void>;
  closeWindow(windowId: ClipboardWindowId): Promise<void>;
}

export interface RuntimePort {
  invoke(command: string, args?: Record<string, unknown>): Promise<unknown>;
  listen(event: string, handler: (payload: unknown) => void): Promise<Unlisten>;
}
