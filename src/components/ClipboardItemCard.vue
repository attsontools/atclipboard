<script setup lang="ts">
import type { ClipboardItem } from '../frontend/types';

const props = defineProps<{ item: ClipboardItem }>();
const emit = defineEmits<{
  restore: [id: string];
  preview: [id: string];
  pin: [id: string, isPinned: boolean];
  delete: [id: string];
}>();

const kindLabels: Record<ClipboardItem['kind'], string> = {
  text: '文本',
  image: '图片',
  files: '文件',
};

function formatTime(value: string) {
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return new Intl.DateTimeFormat('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  }).format(parsed);
}
</script>

<template>
  <article class="clipboard-card">
    <button class="clipboard-card__main" type="button" title="恢复到剪贴板" @click="emit('restore', props.item.id)">
      <div class="clipboard-card__meta">
        <span class="clipboard-card__kind">{{ kindLabels[props.item.kind] }}</span>
        <span>{{ formatTime(props.item.createdAt) }}</span>
      </div>
      <div v-if="props.item.kind === 'image'" class="clipboard-card__body">
        <img v-if="props.item.previewDataUrl" class="clipboard-card__thumb" :src="props.item.previewDataUrl" alt="剪贴板图片预览" />
        <span v-else>{{ props.item.preview }}</span>
      </div>
      <div v-else-if="props.item.kind === 'files'" class="clipboard-card__body clipboard-card__body--files">
        <strong>{{ props.item.preview }}</strong>
        <span>{{ props.item.filePaths[0] || '文件路径不可用' }}</span>
      </div>
      <div v-else class="clipboard-card__body">{{ props.item.preview || props.item.contentText }}</div>
    </button>
    <div class="clipboard-card__actions">
      <button class="card-action" type="button" title="预览" aria-label="预览" @click="emit('preview', props.item.id)">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12Z"/><circle cx="12" cy="12" r="2.5"/></svg>
      </button>
      <button class="card-action" :class="{ 'card-action--active': props.item.isPinned }" type="button" :title="props.item.isPinned ? '取消收藏' : '收藏'" :aria-label="props.item.isPinned ? '取消收藏' : '收藏'" @click="emit('pin', props.item.id, !props.item.isPinned)">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1.1 6.2-5.6-2.9-5.6 2.9 1.1-6.2L3 9.6l6.2-.9L12 3Z"/></svg>
      </button>
      <button class="card-action card-action--danger" type="button" title="删除" aria-label="删除" @click="emit('delete', props.item.id)">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16M9 7V4h6v3m-8 0 1 13h8l1-13M10 11v5m4-5v5"/></svg>
      </button>
    </div>
  </article>
</template>
