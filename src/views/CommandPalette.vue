<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import { AppInput, AppModal } from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'

const ui = useUiStore()
const tabs = useTabsStore()
const query = ref('')
const selected = ref(0)
const input = ref<{ focus: () => void } | null>(null)
const show = ref(false)

const commands = computed(() => [
  { id: 'new', label: '新建快捷请求', hint: 'Ctrl+T', run: () => tabs.openQuick() },
  { id: 'history', label: '打开请求历史', hint: '', run: () => { ui.historyOpen = true } },
  { id: 'sidebar', label: ui.sidebarVisible ? '隐藏侧边栏' : '显示侧边栏', hint: 'Ctrl+B', run: () => { ui.sidebarVisible = !ui.sidebarVisible } },
  { id: 'layout', label: '切换请求/响应布局', hint: 'Ctrl+Shift+L', run: () => ui.cycleLayout() },
  { id: 'theme', label: ui.isDark ? '切换到亮色主题' : '切换到暗色主题', hint: '', run: () => { ui.themeMode = ui.isDark ? 'light' : 'dark'; ui.applyTheme() } },
  { id: 'settings', label: '打开设置', hint: 'Ctrl+,', run: () => tabs.openSettings() },
])

const filtered = computed(() => {
  const needle = query.value.trim().toLowerCase()
  return needle ? commands.value.filter((command) => command.label.toLowerCase().includes(needle)) : commands.value
})

function run(index = selected.value) {
  const command = filtered.value[index]
  if (!command) return
  ui.paletteOpen = false
  command.run()
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') { event.preventDefault(); ui.paletteOpen = false }
  else if (event.key === 'ArrowDown') { event.preventDefault(); selected.value = (selected.value + 1) % Math.max(1, filtered.value.length) }
  else if (event.key === 'ArrowUp') { event.preventDefault(); selected.value = (selected.value - 1 + Math.max(1, filtered.value.length)) % Math.max(1, filtered.value.length) }
  else if (event.key === 'Enter') { event.preventDefault(); run() }
}

watch(
  () => ui.paletteOpen,
  (open) => {
    show.value = open
  },
)
watch(show, (open) => {
  if (!open && ui.paletteOpen) ui.paletteOpen = false
})

onMounted(() => {
  void nextTick(() => { show.value = ui.paletteOpen })
})

function focusInput() {
  void nextTick(() => input.value?.focus())
}
</script>

<template>
  <NaiveThemeScope>
    <AppModal
      v-model:show="show"
      preset="card"
      class="palette-panel"
      :style="{ width: '560px', maxWidth: 'calc(100vw - 24px)' }"
      :bordered="false"
      :closable="false"
      :auto-focus="false"
      aria-label="命令面板"
      @after-enter="focusInput"
      @keydown="onKeydown"
    >
        <AppInput ref="input" v-model:value="query" class="palette-input" size="medium" placeholder="搜索或输入命令" @update:value="selected = 0" />
        <div class="commands" role="listbox">
          <button v-for="(command, index) in filtered" :key="command.id" type="button" role="option" :aria-selected="selected === index" :class="{ selected: selected === index }" @mouseenter="selected = index" @click="run(index)">
            <span>{{ command.label }}</span><kbd v-if="command.hint">{{ command.hint }}</kbd>
          </button>
          <p v-if="filtered.length === 0">没有匹配的命令</p>
        </div>
    </AppModal>
  </NaiveThemeScope>
</template>

<style scoped>
.palette-panel :deep(.n-card) { overflow: hidden; }
.palette-panel :deep(.n-card__content) { padding: 0; }
.palette-input { width: 100%; }
.palette-input :deep(.n-input-wrapper) { padding: 7px 12px; }
.palette-input :deep(.n-input__input-el) { height: 32px; }
.commands { max-height: 360px; overflow: auto; padding: 5px; }
.commands button { width: 100%; height: 36px; display: flex; align-items: center; justify-content: space-between; padding: 0 9px; background: transparent; border: 0; border-radius: var(--a-radius-sm); color: var(--a-text); font: var(--a-fs-sm) var(--a-font-ui); cursor: pointer; }
.commands button.selected { background: var(--a-selected); color: var(--a-primary); }
.commands kbd { color: var(--a-text-faint); font: var(--a-fs-xs) var(--a-font-mono); }
.commands p { padding: 18px; text-align: center; color: var(--a-text-faint); }
</style>
