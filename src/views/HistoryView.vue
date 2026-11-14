<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue';

import ClipboardItemCard from '../components/ClipboardItemCard.vue';
import StateView from '../components/StateView.vue';
import { createClipboardStore } from '../frontend/clipboardStore';
import { messageFromUnknown } from '../frontend/errors';
import { createStandaloneAdapter } from '../frontend/standaloneAdapter';
import { createThemeController } from '../frontend/theme';
import type { ClipboardFilterKind } from '../frontend/types';

const adapter = createStandaloneAdapter();
const store = createClipboardStore(adapter);
const theme = createThemeController();
const searchDraft = ref('');
const rail = ref<HTMLElement | null>(null);
const notice = ref('');

function close() {
  void adapter.closeWindow('history');
}

async function restore(id: string) {
  try {
    await store.restore(id);
    notice.value = '已恢复到系统剪贴板';
    window.setTimeout(close, 250);
  } catch (reason) {
    notice.value = messageFromUnknown(reason);
  }
}

async function mutate(action: () => Promise<unknown>, success: string) {
  notice.value = '';
  try {
    await action();
    notice.value = success;
  } catch (reason) {
    notice.value = messageFromUnknown(reason);
  }
}

function openPreview(id: string) {
  void mutate(() => store.openPreview(id), '');
}

function setPinned(id: string, isPinned: boolean) {
  void mutate(() => store.setPinned(id, isPinned), isPinned ? '已收藏' : '已取消收藏');
}

function deleteItem(id: string) {
  void mutate(() => store.deleteItem(id), '记录已删除');
}

function applyKind(kind: ClipboardFilterKind) {
  void store.setFilters(kind, searchDraft.value);
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') close();
}

function onWheel(event: WheelEvent) {
  if (!rail.value || Math.abs(event.deltaY) <= Math.abs(event.deltaX)) return;
  event.preventDefault();
  rail.value.scrollLeft += event.deltaY;
}

onMounted(() => {
  void theme.start();
  void store.startHistory();
  window.addEventListener('keydown', onKeydown);
});

onUnmounted(() => {
  store.destroy();
  theme.destroy();
  window.removeEventListener('keydown', onKeydown);
});
</script>

<template>
  <div class="app-root history-app">
    <main class="history-panel">
      <header class="history-panel__header">
        <h1>剪贴板历史</h1>
        <div class="appbar__actions">
          <span v-if="notice" class="appbar__count" aria-live="polite">{{ notice }}</span>
          <button class="button button--icon button--ghost" type="button" title="关闭" aria-label="关闭" @click="close">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg>
          </button>
        </div>
      </header>
      <div class="toolbar">
        <div class="toolbar__search">
          <input v-model="searchDraft" class="text-input" type="search" placeholder="搜索剪贴板历史" @keyup.enter="store.setFilters(store.kind.value, searchDraft)">
          <button class="button" type="button" @click="store.setFilters(store.kind.value, searchDraft)">搜索</button>
        </div>
        <select class="select-input" :value="store.kind.value" aria-label="内容类型" @change="applyKind(($event.target as HTMLSelectElement).value as ClipboardFilterKind)">
          <option value="all">全部</option><option value="text">文本</option><option value="image">图片</option><option value="files">文件</option>
        </select>
        <button class="button button--icon" type="button" title="刷新" aria-label="刷新" @click="store.refresh">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 6v5h-5M4 18v-5h5M6.2 8.8A7 7 0 0 1 18.5 7L20 9M4 15l1.5 2A7 7 0 0 0 17.8 15.2"/></svg>
        </button>
      </div>

      <div ref="rail" class="history-panel__rail" @wheel="onWheel">
        <StateView v-if="store.loading.value && store.items.value.length === 0" state="loading" />
        <StateView v-else-if="store.error.value && store.items.value.length === 0" state="error" :message="store.error.value" @retry="store.retryHistory" />
        <StateView v-else-if="store.items.value.length === 0" state="empty" />
        <div v-else class="history-panel__list">
          <ClipboardItemCard v-for="item in store.items.value" :key="item.id" :item="item" @restore="restore" @preview="openPreview" @pin="setPinned" @delete="deleteItem" />
          <div v-if="store.nextCursor.value" class="history-panel__more">
            <button class="button" type="button" :disabled="store.loadingMore.value" @click="store.loadMore">更多</button>
          </div>
        </div>
      </div>
    </main>
  </div>
</template>
