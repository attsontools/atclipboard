<script setup lang="ts">
defineProps<{
  state: 'loading' | 'empty' | 'error';
  message?: string | null;
}>();
defineEmits<{ retry: [] }>();
</script>

<template>
  <div class="state-view" role="status">
    <div v-if="state === 'loading'" class="skeleton" aria-label="正在加载" />
    <div v-else class="state-view__content">
      <span class="state-view__icon" aria-hidden="true">
        <svg v-if="state === 'empty'" viewBox="0 0 24 24"><path d="M4 7h16v13H4zM8 4h8v3M8 12h8"/></svg>
        <svg v-else viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/><path d="M12 7v6m0 4h.01"/></svg>
      </span>
      <p>{{ state === 'empty' ? (message || '还没有剪贴板记录') : (message || '后台服务暂时不可用') }}</p>
      <button v-if="state === 'error'" class="button" type="button" @click="$emit('retry')">重试</button>
    </div>
  </div>
</template>
