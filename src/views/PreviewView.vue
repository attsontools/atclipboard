<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue';

import StateView from '../components/StateView.vue';
import { createClipboardStore } from '../frontend/clipboardStore';
import { messageFromUnknown } from '../frontend/errors';
import { createStandaloneAdapter } from '../frontend/standaloneAdapter';
import { createThemeController } from '../frontend/theme';

const adapter = createStandaloneAdapter();
const store = createClipboardStore(adapter);
const theme = createThemeController();
const notice = ref('');

const kindLabel = computed(() => ({ text: '文本', image: '图片', files: '文件' }[store.previewItem.value?.kind ?? 'text']));
const createdAt = computed(() => {
  const value = store.previewItem.value?.createdAt;
  if (!value) return '';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : new Intl.DateTimeFormat('zh-CN', { dateStyle: 'medium', timeStyle: 'short' }).format(parsed);
});

function close() {
  void adapter.closeWindow('preview');
}

async function restore() {
  const id = store.previewItem.value?.id;
  if (!id) return;
  notice.value = '';
  try {
    await store.restore(id);
    notice.value = '已恢复到系统剪贴板';
  } catch (reason) {
    notice.value = messageFromUnknown(reason);
  }
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') close();
}

onMounted(() => {
  void theme.start();
  void store.startPreview();
  window.addEventListener('keydown', onKeydown);
});

onUnmounted(() => {
  store.destroy();
  theme.destroy();
  window.removeEventListener('keydown', onKeydown);
});
</script>

<template>
  <div class="app-root preview-app">
    <main class="preview-page">
      <header class="preview-header">
        <div>
          <h1>剪贴板{{ kindLabel }}</h1>
          <p>{{ createdAt || '等待预览内容' }}</p>
        </div>
        <button class="button button--icon button--ghost" type="button" title="关闭" aria-label="关闭" @click="close">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18"/></svg>
        </button>
      </header>

      <StateView v-if="store.previewLoading.value" state="loading" />
      <StateView v-else-if="store.previewError.value" state="error" :message="store.previewError.value" @retry="store.retryPreview" />
      <div v-else-if="store.previewItem.value" class="preview-content">
        <img v-if="store.previewItem.value.kind === 'image' && store.previewItem.value.previewDataUrl" class="preview-content__image" :src="store.previewItem.value.previewDataUrl" alt="剪贴板图片预览">
        <ul v-else-if="store.previewItem.value.kind === 'files'" class="preview-content__files">
          <li v-for="path in store.previewItem.value.filePaths" :key="path">{{ path }}</li>
        </ul>
        <pre v-else class="preview-content__text">{{ store.previewItem.value.contentText }}</pre>
      </div>
      <StateView v-else state="empty" message="等待预览内容" />

      <p v-if="store.previewItem.value?.contentTruncated" class="preview-notice">内容过长，当前仅显示有界预览；恢复时仍使用完整原文。</p>
      <footer class="preview-footer">
        <span class="preview-footer__label">{{ notice || store.previewItem.value?.preview || '未选择内容' }}</span>
        <button class="button button--primary" type="button" :disabled="!store.previewItem.value" @click="restore">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 7V5a4 4 0 0 1 8 0v2M6 7h12v13H6z"/></svg>
          恢复到剪贴板
        </button>
      </footer>
    </main>
  </div>
</template>
