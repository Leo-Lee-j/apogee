<script setup lang="ts">
import { nextTick, ref } from 'vue'
import { useAppStore } from '@/stores/app'

const app = useAppStore()
const name = ref('')
const path = ref('')
const nameInput = ref<HTMLInputElement | null>(null)

void nextTick(() => nameInput.value?.focus())

async function chooseDirectory() {
  const { open } = await import('@tauri-apps/plugin-dialog')
  const selected = await open({ directory: true, multiple: false })
  if (typeof selected !== 'string') return
  path.value = selected
  if (!name.value.trim()) name.value = selected.split(/[\\/]/).filter(Boolean).pop() ?? ''
  await nextTick()
  nameInput.value?.focus()
  nameInput.value?.select()
}

async function submit() {
  if (!name.value.trim()) return
  if (await app.createWorkspace(path.value, name.value.trim())) app.createDialogOpen = false
}
</script>

<template>
  <div class="dialog-backdrop" @mousedown.self="app.createDialogOpen = false">
    <form class="dialog" @submit.prevent="submit">
      <header>
        <h2>新建工作区</h2>
        <button type="button" aria-label="关闭" @click="app.createDialogOpen = false">×</button>
      </header>
      <label>
        <span>名称</span>
        <input ref="nameInput" v-model="name" maxlength="120" autocomplete="off" placeholder="例如：用户服务" />
      </label>
      <label>
        <span>位置</span>
        <div class="path-picker">
          <input :value="path || `默认位置：${app.settings.defaultWorkspaceRoot}`" readonly @click="chooseDirectory" />
          <button type="button" @click="chooseDirectory">选择其他位置</button>
        </div>
      </label>
      <p v-if="app.lastError" class="error">{{ app.lastError.message }}</p>
      <footer>
        <button type="button" @click="app.createDialogOpen = false">取消</button>
        <button class="primary" type="submit" :disabled="!name.trim() || app.shell === 'opening'">创建</button>
      </footer>
    </form>
  </div>
</template>

<style scoped>
.dialog-backdrop { position: fixed; inset: 0; z-index: 100; display: grid; place-items: center; background: rgba(7, 10, 16, .48); }
.dialog { width: min(440px, calc(100vw - 32px)); padding: 20px; border: 1px solid var(--a-border-strong); border-radius: var(--a-radius-lg); background: var(--a-bg-2); box-shadow: var(--a-shadow-lg); }
header { display: flex; align-items: center; justify-content: space-between; margin-bottom: 20px; }
h2 { margin: 0; font-size: 15px; }
header button { width: 28px; height: 28px; border: 0; background: transparent; color: var(--a-text-dim); font: 20px/1 var(--a-font-ui); cursor: pointer; }
label { display: block; margin-top: 14px; }
label > span { display: block; margin-bottom: 6px; color: var(--a-text-dim); font-size: 12px; }
input { width: 100%; height: 34px; padding: 0 10px; border: 1px solid var(--a-border-strong); border-radius: var(--a-radius-md); outline: 0; background: var(--a-bg-3); color: var(--a-text); font: inherit; }
input:focus { border-color: var(--a-focus); box-shadow: var(--a-focus-halo); }
.path-picker { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 8px; }
button { height: 32px; padding: 0 14px; border: 1px solid var(--a-border-strong); border-radius: var(--a-radius-md); background: var(--a-bg-3); color: var(--a-text); font: inherit; cursor: pointer; }
button:hover { background: var(--a-hover); }
button.primary { border-color: var(--a-primary); background: var(--a-primary); color: var(--a-on-primary); }
button:disabled { cursor: default; opacity: .5; }
.error { margin: 12px 0 0; color: var(--a-error); font-size: 12px; }
footer { display: flex; justify-content: flex-end; gap: 8px; margin-top: 22px; }
</style>
