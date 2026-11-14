import { ref } from 'vue';

import { messageFromUnknown } from './errors';
import type {
  ClipboardAdapter,
  ClipboardFilterKind,
  ClipboardItem,
  ClipboardSettings,
  ClipboardSettingsState,
  Unlisten,
} from './types';

const PAGE_SIZE = 50;

export function createClipboardStore(adapter: ClipboardAdapter) {
  const items = ref<ClipboardItem[]>([]);
  const nextCursor = ref<string | null>(null);
  const kind = ref<ClipboardFilterKind>('all');
  const query = ref('');
  const loading = ref(false);
  const loadingMore = ref(false);
  const error = ref<string | null>(null);
  const settings = ref<ClipboardSettingsState | null>(null);
  const settingsLoading = ref(false);
  const settingsError = ref<string | null>(null);
  const previewItem = ref<ClipboardItem | null>(null);
  const previewLoading = ref(false);
  const previewError = ref<string | null>(null);
  const unlisteners: Unlisten[] = [];
  let historyGeneration = 0;
  let paginationGeneration = 0;
  let previewGeneration = 0;
  let activationSequence = 0;
  let previewId: string | null = null;
  let includeSettings = false;
  let historyStarted = false;
  let previewStarted = false;

  function listParams(cursor: string | null) {
    return {
      kind: kind.value === 'all' ? null : kind.value,
      query: query.value,
      cursor,
      limit: PAGE_SIZE,
    };
  }

  async function refresh() {
    const generation = ++historyGeneration;
    paginationGeneration += 1;
    loadingMore.value = false;
    loading.value = true;
    error.value = null;
    try {
      const result = await adapter.list(listParams(null));
      if (generation !== historyGeneration) return;
      items.value = result.items;
      nextCursor.value = result.nextCursor;
    } catch (caught) {
      if (generation === historyGeneration) error.value = messageFromUnknown(caught);
    } finally {
      if (generation === historyGeneration) loading.value = false;
    }
  }

  async function loadMore() {
    if (!nextCursor.value || loading.value || loadingMore.value) return;
    const cursor = nextCursor.value;
    const history = historyGeneration;
    const pagination = ++paginationGeneration;
    loadingMore.value = true;
    error.value = null;
    try {
      const result = await adapter.list(listParams(cursor));
      if (history !== historyGeneration || pagination !== paginationGeneration) return;
      const known = new Set(items.value.map(({ id }) => id));
      items.value = [...items.value, ...result.items.filter(({ id }) => !known.has(id))];
      nextCursor.value = result.nextCursor;
    } catch (caught) {
      if (pagination === paginationGeneration) error.value = messageFromUnknown(caught);
    } finally {
      if (pagination === paginationGeneration) loadingMore.value = false;
    }
  }

  async function setFilters(nextKind: ClipboardFilterKind, nextQuery: string) {
    kind.value = nextKind;
    query.value = nextQuery;
    await refresh();
  }

  async function loadSettings() {
    settingsLoading.value = true;
    settingsError.value = null;
    try {
      settings.value = await adapter.getSettings();
    } catch (caught) {
      settingsError.value = messageFromUnknown(caught);
    } finally {
      settingsLoading.value = false;
    }
  }

  async function startHistory(options: { includeSettings?: boolean } = {}) {
    if (options.includeSettings !== undefined) includeSettings = options.includeSettings;
    if (historyStarted) {
      await retryHistory();
      return;
    }
    error.value = null;
    try {
      await adapter.connect();
    } catch (caught) {
      error.value = messageFromUnknown(caught);
      loading.value = false;
      return;
    }
    try {
      unlisteners.push(await adapter.onHistoryChanged(() => {
        void refresh();
        if (includeSettings) void loadSettings();
      }));
    } catch (caught) {
      error.value = messageFromUnknown(caught);
      return;
    }
    historyStarted = true;
    await Promise.all([refresh(), includeSettings ? loadSettings() : Promise.resolve()]);
  }

  async function retryHistory() {
    if (!historyStarted) return startHistory({ includeSettings });
    error.value = null;
    try {
      await adapter.connect();
    } catch (caught) {
      error.value = messageFromUnknown(caught);
      return;
    }
    await Promise.all([refresh(), includeSettings ? loadSettings() : Promise.resolve()]);
  }

  async function saveSettings(next: ClipboardSettings) {
    const previous = settings.value ? { ...settings.value } : null;
    settings.value = previous ? { ...previous, ...next } : null;
    settingsError.value = null;
    try {
      settings.value = await adapter.updateSettings(next);
    } catch (caught) {
      settings.value = previous;
      settingsError.value = messageFromUnknown(caught);
      throw caught;
    }
  }

  async function deleteItem(id: string) {
    await adapter.delete(id);
    await refresh();
  }

  async function setPinned(id: string, isPinned: boolean) {
    await adapter.pin(id, isPinned);
    await refresh();
  }

  async function clearUnpinned() {
    const deletedCount = await adapter.clearUnpinned();
    await refresh();
    return deletedCount;
  }

  async function restore(id: string) {
    await adapter.restore(id);
  }

  function openPreview(id: string) {
    return adapter.openWindow('preview', { id });
  }

  async function loadPreview(id: string) {
    previewId = id;
    const generation = ++previewGeneration;
    previewLoading.value = true;
    previewError.value = null;
    try {
      const loaded = await adapter.get(id);
      if (generation === previewGeneration) previewItem.value = loaded;
    } catch (caught) {
      if (generation === previewGeneration) {
        previewItem.value = null;
        previewError.value = messageFromUnknown(caught);
      }
    } finally {
      if (generation === previewGeneration) previewLoading.value = false;
    }
  }

  async function loadCurrentPreviewActivation() {
    const sequence = activationSequence;
    const activation = await adapter.getWindowActivation('preview');
    if (activation && sequence === activationSequence) await loadPreview(activation.id);
  }

  async function retryPreview() {
    if (!previewStarted) return startPreview();
    previewError.value = null;
    try {
      await adapter.connect();
      if (previewId) {
        await loadPreview(previewId);
        return;
      }
      await loadCurrentPreviewActivation();
    } catch (caught) {
      previewError.value = messageFromUnknown(caught);
    }
  }

  async function startPreview() {
    try {
      await adapter.connect();
    } catch (caught) {
      previewError.value = messageFromUnknown(caught);
      previewLoading.value = false;
      return;
    }
    const registered: Unlisten[] = [];
    try {
      registered.push(await adapter.onWindowActivation('preview', ({ id }) => {
        activationSequence += 1;
        void loadPreview(id);
      }));
      registered.push(await adapter.onHistoryChanged(() => {
        if (previewId) void loadPreview(previewId);
      }));
    } catch (caught) {
      for (const unlisten of registered) unlisten();
      previewError.value = messageFromUnknown(caught);
      return;
    }
    unlisteners.push(...registered);
    previewStarted = true;
    try {
      await loadCurrentPreviewActivation();
    } catch (caught) {
      previewError.value = messageFromUnknown(caught);
    }
  }

  function destroy() {
    for (const unlisten of unlisteners.splice(0)) unlisten();
  }

  return {
    items,
    nextCursor,
    kind,
    query,
    loading,
    loadingMore,
    error,
    settings,
    settingsLoading,
    settingsError,
    previewItem,
    previewLoading,
    previewError,
    startHistory,
    retryHistory,
    refresh,
    loadMore,
    setFilters,
    loadSettings,
    saveSettings,
    deleteItem,
    setPinned,
    clearUnpinned,
    restore,
    openPreview,
    loadPreview,
    retryPreview,
    startPreview,
    destroy,
  };
}

export type ClipboardStore = ReturnType<typeof createClipboardStore>;
