<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { AppIcon } from '@/components/ui'
import { IconStats } from '@/components/ui/icons'
import { inTauri, ipc, type AppMemInfo, type AppMemoryPartKind } from '@/ipc'
import { useAppStore } from '@/stores/app'

const SAMPLE_INTERVAL_MS = 2_000
const app = useAppStore()
const memory = ref<AppMemInfo | null>(null)
const loading = ref(false)
let timer: ReturnType<typeof setInterval> | null = null

const partLabels: Record<AppMemoryPartKind, string> = {
  main: 'Apogee 主进程',
  webview: 'WebView 渲染进程',
  other: '其他子进程',
}

const label = computed(() => {
  if (!inTauri || !memory.value?.available) return '内存不可用'
  return `内存 ${formatBytes(memory.value.privateBytes)}`
})

function formatBytes(bytes: number) {
  return `${(bytes / 1048576).toFixed(bytes >= 104857600 ? 0 : 1)} MB`
}

async function sample() {
  if (loading.value || !app.settings.showMemoryMonitor || !inTauri) return
  loading.value = true
  try {
    memory.value = await ipc.appMemory()
  } catch {
    memory.value = null
  } finally {
    loading.value = false
  }
}

function stopSampling() {
  if (!timer) return
  clearInterval(timer)
  timer = null
}

function startSampling() {
  if (timer || !inTauri) return
  void sample()
  timer = setInterval(() => void sample(), SAMPLE_INTERVAL_MS)
}

watch(() => app.settings.showMemoryMonitor, (visible) => {
  if (visible) startSampling()
  else {
    stopSampling()
    memory.value = null
  }
}, { immediate: true })

onBeforeUnmount(stopSampling)
</script>

<template>
  <div class="memory-monitor-wrap">
    <button class="memory-monitor" type="button" :aria-label="label" :title="label">
      <AppIcon :component="IconStats" :size="14" aria-hidden="true" />
      <span>{{ label }}</span>
    </button>

    <section class="memory-popover" role="tooltip" aria-label="应用内存详情">
      <div class="memory-heading">
        <span>应用内存</span>
        <strong>{{ memory?.available ? formatBytes(memory.privateBytes) : '不可用' }}</strong>
      </div>
      <p v-if="memory?.available" class="memory-caption">
        私有内存 · {{ memory.processCount }} 个进程
      </p>
      <div v-if="memory?.available" class="memory-parts">
        <div v-for="part in memory.parts" :key="part.kind" class="memory-part">
          <span>{{ partLabels[part.kind] }}<em v-if="part.processCount > 1"> ×{{ part.processCount }}</em></span>
          <strong>{{ formatBytes(part.privateBytes) }}</strong>
        </div>
      </div>
      <p v-else class="memory-caption">当前平台或进程权限不支持读取应用内存。</p>
    </section>
  </div>
</template>

<style scoped>
.memory-monitor {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  height: 22px;
  padding: 0 5px;
  border: 0;
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text-faint);
  font: inherit;
  font-variant-numeric: tabular-nums;
  cursor: default;
}
.memory-monitor:hover,
.memory-monitor:focus-visible { color: var(--a-text); background: var(--a-hover); outline: none; }
.memory-monitor-wrap { position: relative; display: inline-flex; }
.memory-popover {
  position: absolute;
  right: 0;
  bottom: calc(100% + 8px);
  z-index: 20;
  min-width: 226px;
  padding: 10px;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-2);
  box-shadow: var(--a-shadow-md);
  color: var(--a-text);
  font-size: 12px;
  opacity: 0;
  pointer-events: none;
  transform: translateY(3px);
  transition: opacity var(--a-dur) var(--a-ease), transform var(--a-dur) var(--a-ease);
}
.memory-monitor-wrap:hover .memory-popover,
.memory-monitor-wrap:focus-within .memory-popover { opacity: 1; pointer-events: auto; transform: translateY(0); }
.memory-heading, .memory-part { display: flex; align-items: baseline; justify-content: space-between; gap: 20px; }
.memory-heading { font-size: 13px; font-weight: var(--a-fw-semibold); }
.memory-heading strong, .memory-part strong { font-family: var(--a-font-mono); font-variant-numeric: tabular-nums; }
.memory-caption { margin: 5px 0 8px; color: var(--a-text-faint); line-height: 18px; }
.memory-parts { display: grid; gap: 5px; padding-top: 8px; border-top: 1px solid var(--a-border); }
.memory-part { color: var(--a-text-dim); }
.memory-part strong { color: var(--a-text); font-weight: var(--a-fw-medium); }
.memory-part em { color: var(--a-text-faint); font-style: normal; }
</style>
