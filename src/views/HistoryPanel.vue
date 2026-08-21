<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { AppAlert, AppButton, AppCard, AppInput, AppModal, AppPopconfirm } from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { ipc, normalizeError, type HistoryEntry } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'

const ui = useUiStore()
const tabs = useTabsStore()
const entries = ref<HistoryEntry[]>([])
const query = ref('')
const loading = ref(false)
const error = ref('')
const show = ref(false)

watch(() => ui.historyOpen, (open) => { show.value = open })
watch(show, (open) => {
  if (!open && ui.historyOpen) ui.historyOpen = false
})

const filtered = computed(() => {
  const needle = query.value.trim().toLowerCase()
  return needle
    ? entries.value.filter((entry) => `${entry.method} ${entry.url} ${entry.status ?? ''}`.toLowerCase().includes(needle))
    : entries.value
})

async function load() {
  loading.value = true
  error.value = ''
  try { entries.value = await ipc.historyList() }
  catch (reason) { error.value = normalizeError(reason).message }
  finally { loading.value = false }
}

async function open(entry: HistoryEntry) {
  loading.value = true
  error.value = ''
  try {
    const record = await ipc.historyRead(entry.id)
    const spec = record.spec
    tabs.open({
      title: historyTitle(entry), method: spec.method, url: spec.url,
      params: spec.params ?? [], pathParams: spec.pathParams ?? [], headers: spec.headers ?? [],
      body: spec.body ?? { type: 'none' }, auth: spec.auth ?? { type: 'none' }, settings: spec.settings ?? {},
      response: record.response ?? null, error: record.error ?? null,
      actualRequest: record.actualRequest ?? null, dirty: false, preview: false,
    })
    ui.historyOpen = false
  } catch (reason) { error.value = normalizeError(reason).message }
  finally { loading.value = false }
}

async function clearHistory() {
  loading.value = true
  try { await ipc.historyClear(); entries.value = [] }
  catch (reason) { error.value = normalizeError(reason).message }
  finally { loading.value = false }
}

function historyTitle(entry: HistoryEntry) {
  try { return new URL(entry.url).pathname.split('/').filter(Boolean).pop() || new URL(entry.url).hostname }
  catch { return '历史请求' }
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

function formatTime(ms: number) {
  return new Intl.DateTimeFormat('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' }).format(ms)
}

onMounted(() => {
  void nextTick(() => { show.value = ui.historyOpen })
  void load()
})
</script>

<template>
  <NaiveThemeScope>
    <AppModal v-model:show="show">
      <AppCard class="history-card" title="请求历史" :bordered="false" size="small" role="dialog">
        <div class="toolbar">
          <AppInput v-model:value="query" clearable placeholder="搜索方法、URL 或状态码" />
          <AppPopconfirm positive-text="清空" negative-text="取消" @positive-click="clearHistory">
            <template #trigger><AppButton secondary type="error" :disabled="entries.length === 0">清空</AppButton></template>
            将删除全部请求历史及响应快照。
          </AppPopconfirm>
        </div>
        <AppAlert v-if="error" type="error" :show-icon="false">{{ error }}</AppAlert>
        <div class="history-list" :aria-busy="loading">
          <button v-for="entry in filtered" :key="entry.id" type="button" class="history-row" @click="open(entry)">
            <span class="method">{{ entry.method }}</span>
            <span class="url">{{ entry.url }}</span>
            <span class="status" :class="{ bad: (entry.status ?? 0) >= 400 }">{{ entry.status ?? 'ERR' }}</span>
            <span class="meta">{{ entry.durationMs.toFixed(0) }} ms · {{ formatBytes(entry.sizeBytes) }}</span>
            <time :datetime="new Date(entry.createdAtMs).toISOString()">{{ formatTime(entry.createdAtMs) }}</time>
          </button>
          <p v-if="!loading && filtered.length === 0" class="empty">没有匹配的请求历史。</p>
        </div>
      </AppCard>
    </AppModal>
  </NaiveThemeScope>
</template>

<style scoped>
.history-card { width: min(860px, calc(100vw - 32px)); }
.toolbar { display: grid; grid-template-columns: 1fr auto; gap: 8px; margin-bottom: 8px; }
.history-list { max-height: min(62vh, 560px); overflow: auto; border-top: 1px solid var(--a-border); }
.history-row { width: 100%; display: grid; grid-template-columns: 64px minmax(180px, 1fr) 52px 130px 142px; align-items: center; gap: 8px; min-height: var(--a-h-list-row); padding: 5px 8px; background: transparent; border: 0; border-bottom: 1px solid var(--a-border); color: var(--a-text); text-align: left; cursor: pointer; font: var(--a-fs-sm)/var(--a-lh-sm) var(--a-font-ui); }
.history-row:hover { background: var(--a-hover); }
.method { color: var(--a-primary); font: var(--a-fw-semibold) var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-mono); }
.url { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--a-font-mono); }
.status.bad { color: var(--a-error); }
.meta, time { color: var(--a-text-faint); white-space: nowrap; }
.empty { padding: 28px; text-align: center; color: var(--a-text-faint); }
@media (max-width: 700px) { .history-row { grid-template-columns: 54px 1fr 44px; } .meta, time { display: none; } }
</style>
