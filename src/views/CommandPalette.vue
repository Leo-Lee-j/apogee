<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'

const ui = useUiStore()
const tabs = useTabsStore()
const query = ref('')
const selected = ref(0)
const input = ref<HTMLInputElement | null>(null)

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

onMounted(() => nextTick(() => input.value?.focus()))
</script>

<template>
  <Teleport to="body">
    <div class="palette-backdrop" role="presentation" @mousedown.self="ui.paletteOpen = false">
      <section class="palette-panel" role="dialog" aria-label="命令面板" @keydown="onKeydown">
        <input ref="input" v-model="query" class="palette-input" placeholder="搜索或输入命令" @input="selected = 0" />
        <div class="commands" role="listbox">
          <button v-for="(command, index) in filtered" :key="command.id" type="button" role="option" :aria-selected="selected === index" :class="{ selected: selected === index }" @mouseenter="selected = index" @click="run(index)">
            <span>{{ command.label }}</span><kbd v-if="command.hint">{{ command.hint }}</kbd>
          </button>
          <p v-if="filtered.length === 0">没有匹配的命令</p>
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.palette-backdrop { position: fixed; inset: 0; z-index: 3000; display: flex; justify-content: center; align-items: flex-start; padding-top: min(18vh, 150px); background: rgb(0 0 0 / 38%); }
.palette-panel { width: min(560px, calc(100vw - 24px)); overflow: hidden; background: var(--a-bg-2); border: 1px solid var(--a-border-strong); border-radius: var(--a-radius-md); box-shadow: var(--a-shadow-lg); }
.palette-input { width: 100%; height: 46px; box-sizing: border-box; padding: 0 14px; background: var(--a-bg-2); border: 0; border-bottom: 1px solid var(--a-border); outline: none; color: var(--a-text); font: var(--a-fs-base) var(--a-font-ui); }
.commands { max-height: 360px; overflow: auto; padding: 5px; }
.commands button { width: 100%; height: 36px; display: flex; align-items: center; justify-content: space-between; padding: 0 9px; background: transparent; border: 0; border-radius: var(--a-radius-sm); color: var(--a-text); font: var(--a-fs-sm) var(--a-font-ui); cursor: pointer; }
.commands button.selected { background: var(--a-selected); color: var(--a-primary); }
.commands kbd { color: var(--a-text-faint); font: var(--a-fs-xs) var(--a-font-mono); }
.commands p { padding: 18px; text-align: center; color: var(--a-text-faint); }
</style>
