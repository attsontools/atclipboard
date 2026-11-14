<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core';
import { onMounted, ref } from 'vue';

interface ClipboardItem {
  id: string;
  kind: 'text' | 'image' | 'files';
  preview: string;
  contentText: string;
  isPinned: boolean;
  createdAt: string;
}

const items = ref<ClipboardItem[]>([]);
const loading = ref(true);
const error = ref('');

async function loadHistory() {
  loading.value = true;
  error.value = '';
  try {
    await invoke('connect_clipboard_service');
    const result = await invoke<{ items: ClipboardItem[] }>('clipboard_service_request', {
      method: 'clipboard.history.list',
      params: { limit: 50 }
    });
    items.value = result.items;
  } catch (reason) {
    error.value = String(reason);
  } finally {
    loading.value = false;
  }
}

onMounted(loadHistory);
</script>

<template>
  <main class="clipboard-app">
    <header class="appbar">
      <div>
        <h1>剪贴板</h1>
        <span>{{ items.length }} 条历史</span>
      </div>
      <button type="button" title="刷新" aria-label="刷新" @click="loadHistory">
        <svg viewBox="0 0 24 24" aria-hidden="true">
          <path d="M20 6v5h-5M4 18v-5h5M6.1 9a7 7 0 0 1 11.8-2.4L20 9M4 15l2.1 2.4A7 7 0 0 0 17.9 15" />
        </svg>
      </button>
    </header>
    <section class="history" aria-live="polite">
      <p v-if="loading" class="state">正在读取剪贴板历史</p>
      <p v-else-if="error" class="state error">{{ error }}</p>
      <p v-else-if="items.length === 0" class="state">暂无剪贴板历史</p>
      <article v-for="item in items" v-else :key="item.id" class="history-row">
        <span class="kind">{{ item.kind }}</span>
        <div class="content">
          <strong>{{ item.preview }}</strong>
          <span>{{ item.createdAt }}</span>
        </div>
        <span v-if="item.isPinned" class="pinned">已收藏</span>
      </article>
    </section>
  </main>
</template>

<style scoped>
.clipboard-app { min-height: 100vh; background: var(--bg-base); color: var(--text); }
.appbar { height: 64px; padding: 0 20px; display: flex; align-items: center; justify-content: space-between; border-bottom: 1px solid var(--border); background: var(--bg-raised); }
.appbar div { display: flex; align-items: baseline; gap: 12px; }
h1 { margin: 0; font-size: 18px; letter-spacing: 0; }
.appbar span, .content span { color: var(--text-muted); font-size: 12px; }
button { width: 34px; height: 34px; display: grid; place-items: center; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg-control); color: var(--text); cursor: pointer; }
button:hover { border-color: var(--accent); }
button svg { width: 18px; fill: none; stroke: currentColor; stroke-width: 1.8; }
.history { padding: 16px 20px; }
.state { margin: 40px 0; text-align: center; color: var(--text-muted); }
.error { color: var(--danger); }
.history-row { min-height: 58px; display: grid; grid-template-columns: 64px minmax(0, 1fr) auto; gap: 12px; align-items: center; border-bottom: 1px solid var(--border); }
.kind { color: var(--accent); font-size: 12px; text-transform: uppercase; }
.content { min-width: 0; display: flex; flex-direction: column; gap: 4px; }
.content strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px; font-weight: 500; }
.pinned { color: var(--accent); font-size: 12px; }
</style>
