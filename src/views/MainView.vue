<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { adapter, themeBridge } from '@runtime-host';

import ClipboardItemCard from '../components/ClipboardItemCard.vue';
import StateView from '../components/StateView.vue';
import { createClipboardStore } from '../frontend/clipboardStore';
import { messageFromUnknown } from '../frontend/errors';
import { createThemeController } from '../frontend/theme';
import type { ClipboardFilterKind, ClipboardSettings } from '../frontend/types';

const store = createClipboardStore(adapter);
const theme = createThemeController(themeBridge);
const searchDraft = ref('');
const settingsDraft = ref<ClipboardSettings>({
  captureEnabled: true,
  retentionLimit: 500,
  shortcut: 'CommandOrControl+Alt+V',
});
const notice = ref('');
const noticeError = ref(false);
const mutating = ref(false);

const totalLabel = computed(() => `${store.items.value.length}${store.nextCursor.value ? '+' : ''} 条历史`);

watch(store.settings, (value) => {
  if (!value) return;
  settingsDraft.value = {
    captureEnabled: value.captureEnabled,
    retentionLimit: value.retentionLimit,
    shortcut: value.shortcut,
  };
}, { immediate: true, deep: true });

async function runMutation(action: () => Promise<unknown>, success: string) {
  notice.value = '';
  noticeError.value = false;
  mutating.value = true;
  try {
    await action();
    notice.value = success;
  } catch (reason) {
    noticeError.value = true;
    notice.value = messageFromUnknown(reason);
  } finally {
    mutating.value = false;
  }
}

function applySearch() {
  void store.setFilters(store.kind.value, searchDraft.value);
}

function applyKind(kind: ClipboardFilterKind) {
  void store.setFilters(kind, searchDraft.value);
}

function saveSettings() {
  const next = { ...settingsDraft.value };
  void runMutation(() => store.saveSettings(next), '设置已保存');
}

function toggleCapture(event: Event) {
  settingsDraft.value.captureEnabled = (event.target as HTMLInputElement).checked;
  saveSettings();
}

async function clearUnpinned() {
  if (!window.confirm('确认清空所有未收藏的剪贴板历史？收藏项会保留。')) return;
  notice.value = '';
  noticeError.value = false;
  mutating.value = true;
  try {
    const count = await store.clearUnpinned();
    notice.value = `已清空 ${count} 条未收藏记录`;
  } catch (reason) {
    noticeError.value = true;
    notice.value = messageFromUnknown(reason);
  } finally {
    mutating.value = false;
  }
}

function deleteItem(id: string) {
  void runMutation(() => store.deleteItem(id), '记录已删除');
}

function setPinned(id: string, isPinned: boolean) {
  void runMutation(() => store.setPinned(id, isPinned), isPinned ? '已收藏' : '已取消收藏');
}

function restore(id: string) {
  void runMutation(() => store.restore(id), '已恢复到系统剪贴板');
}

function openPreview(id: string) {
  void runMutation(() => store.openPreview(id), '');
}

onMounted(() => {
  void theme.start();
  void store.startHistory({ includeSettings: true });
});

onUnmounted(() => {
  store.destroy();
  theme.destroy();
});
</script>

<template>
  <div class="app-root">
    <header v-if="adapter.mode === 'standalone'" class="appbar">
      <div class="appbar__identity">
        <h1>剪贴板</h1>
        <span class="appbar__count">{{ totalLabel }}</span>
      </div>
      <div class="appbar__actions">
        <button v-if="theme.canToggle" class="button button--icon button--ghost" type="button" :title="theme.theme.value === 'dark' ? '切换到浅色' : '切换到深色'" :aria-label="theme.theme.value === 'dark' ? '切换到浅色' : '切换到深色'" @click="theme.toggle">
          <svg v-if="theme.theme.value === 'dark'" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M4.9 4.9l1.4 1.4m11.4 11.4 1.4 1.4M2 12h2m16 0h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></svg>
          <svg v-else viewBox="0 0 24 24" aria-hidden="true"><path d="M20.5 14.5A8.5 8.5 0 0 1 9.5 3.5 8.7 8.7 0 1 0 20.5 14.5Z"/></svg>
        </button>
        <button class="button button--icon button--ghost" type="button" title="刷新" aria-label="刷新" :disabled="store.loading.value" @click="store.refresh">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 6v5h-5M4 18v-5h5M6.2 8.8A7 7 0 0 1 18.5 7L20 9M4 15l1.5 2A7 7 0 0 0 17.8 15.2"/></svg>
        </button>
      </div>
    </header>

    <main class="app-content">
      <section class="section-band" aria-labelledby="settings-title">
        <div class="section-heading">
          <h2 id="settings-title" class="section-title">
            采集设置
            <span v-if="adapter.mode === 'plugin'" class="section-count">{{ totalLabel }}</span>
          </h2>
          <button
            v-if="adapter.mode === 'plugin'"
            class="button button--icon button--ghost"
            type="button"
            title="刷新"
            aria-label="刷新"
            :disabled="store.loading.value"
            @click="store.refresh"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 6v5h-5M4 18v-5h5M6.2 8.8A7 7 0 0 1 18.5 7L20 9M4 15l1.5 2A7 7 0 0 0 17.8 15.2"/></svg>
          </button>
        </div>
        <div class="settings-row">
          <div class="settings-field">
            <label class="switch" title="启用剪贴板采集">
              <input type="checkbox" :checked="settingsDraft.captureEnabled" :disabled="!store.settings.value || store.settingsLoading.value || mutating" @change="toggleCapture">
              <span class="switch__track" />
            </label>
            <span>{{ settingsDraft.captureEnabled ? '正在采集' : '采集已暂停' }}</span>
          </div>
          <div class="settings-field">
            <label for="retention">保留条数</label>
            <input id="retention" v-model.number="settingsDraft.retentionLimit" class="number-input" type="number" min="50" max="5000" :disabled="!store.settings.value || mutating" @change="saveSettings">
          </div>
          <div class="settings-field settings-field--grow">
            <label for="shortcut">快捷键</label>
            <input id="shortcut" v-model.trim="settingsDraft.shortcut" class="text-input" type="text" :disabled="!store.settings.value || mutating" @keyup.enter="saveSettings">
            <button class="button" type="button" :disabled="!store.settings.value || mutating" @click="saveSettings">保存</button>
          </div>
        </div>
        <div v-if="store.settings.value?.unavailableReason" class="inline-status inline-status--warning">
          {{ store.settings.value.unavailableReason }}
          <code v-if="store.settings.value.systemShortcutCommand">{{ store.settings.value.systemShortcutCommand }}</code>
        </div>
        <div v-else-if="store.settings.value?.shortcutRegisterError || store.settingsError.value" class="inline-status inline-status--error">
          {{ store.settings.value?.shortcutRegisterError || store.settingsError.value }}
        </div>
      </section>

      <section class="section-band" aria-labelledby="history-title">
        <h2 id="history-title" class="section-title">历史记录</h2>
        <div class="toolbar">
          <div class="toolbar__search">
            <input v-model="searchDraft" class="text-input" type="search" placeholder="搜索内容、文件名或路径" @keyup.enter="applySearch">
            <button class="button" type="button" @click="applySearch">搜索</button>
          </div>
          <div class="button-group" aria-label="内容类型">
            <button v-for="option in ([['all', '全部'], ['text', '文本'], ['image', '图片'], ['files', '文件']] as const)" :key="option[0]" class="button" :class="{ 'is-active': store.kind.value === option[0] }" type="button" @click="applyKind(option[0])">{{ option[1] }}</button>
          </div>
          <button class="button button--danger" type="button" :disabled="mutating" @click="clearUnpinned">清空未收藏</button>
        </div>

        <div v-if="notice" class="inline-status" :class="{ 'inline-status--error': noticeError }" aria-live="polite">
          <span class="status-dot" />{{ notice }}
        </div>

        <StateView v-if="store.loading.value && store.items.value.length === 0" state="loading" />
        <StateView v-else-if="store.error.value && store.items.value.length === 0" state="error" :message="store.error.value" @retry="store.retryHistory" />
        <StateView v-else-if="store.items.value.length === 0" state="empty" />
        <template v-else>
          <div class="history-grid">
            <ClipboardItemCard v-for="item in store.items.value" :key="item.id" :item="item" @restore="restore" @preview="openPreview" @pin="setPinned" @delete="deleteItem" />
          </div>
          <div v-if="store.error.value" class="inline-status inline-status--error">{{ store.error.value }}</div>
          <div v-if="store.nextCursor.value" class="load-more">
            <button class="button" type="button" :disabled="store.loadingMore.value" @click="store.loadMore">{{ store.loadingMore.value ? '正在加载' : '加载更多' }}</button>
          </div>
        </template>
      </section>
    </main>
  </div>
</template>
