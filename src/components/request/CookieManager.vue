<script setup lang="ts">
import { onMounted, ref, shallowRef } from 'vue'
import { ipc, normalizeError, type AppError, type SessionCookie } from '@/ipc'
import { AppButton, AppInput, AppModal } from '@/components/ui'
defineEmits<{ close: [] }>()
const cookies = shallowRef<SessionCookie[]>([])
const error = shallowRef<AppError | null>(null)
const busy = ref(false), reveal = ref(false)
const editing = ref<string>(), source = ref(''), header = ref('')
async function run(action: () => Promise<void>) {
  if (busy.value) return
  busy.value = true; error.value = null
  try { await action(); cookies.value = await ipc.workspaceListCookies() }
  catch (failure) { const normalized = normalizeError(failure); error.value = { ...normalized, hints: normalized.hints.length ? normalized.hints : [{ label: '检查来源 URL、Cookie 属性和当前工作区后重试' }] } }
  finally { busy.value = false }
}
function edit(cookie: SessionCookie) {
  editing.value = cookie.id
  source.value = (cookie.secure ? 'https://' : 'http://') + (cookie.domain.includes(':') && !cookie.domain.startsWith('[') ? '[' + cookie.domain + ']' : cookie.domain) + cookie.path
  header.value = cookie.header
}
function reset() { editing.value = undefined; source.value = ''; header.value = '' }
async function save() {
  await run(async () => { await ipc.workspacePutCookie(source.value, header.value, editing.value); reset() })
}
onMounted(() => { void run(async () => {}) })
</script>

<template>
  <AppModal :show="true" preset="card" title="工作区 Cookie 会话" style="width: min(880px, 94vw)" :mask-closable="false" :closable="!busy" :close-on-esc="!busy" @update:show="!$event && $emit('close')">
    <div class="cookie-manager">
      <p>本工作区的 Cookie 仅保留到关闭工作区。最多 300 项、每域 50 项，超限移除最早项。手写 Cookie Header 优先；请求设置可关闭会话的收发。</p>
      <div class="actions">
        <AppButton size="small" :disabled="busy" @click="run(async () => {})">刷新</AppButton>
        <AppButton size="small" @click="reveal = !reveal">{{ reveal ? '隐藏值' : '显示值' }}</AppButton>
        <AppButton size="small" :disabled="busy || !cookies.length" @click="run(async () => { await ipc.workspaceDeleteCookie(); reset() })">清空会话</AppButton>
        <span>{{ cookies.length }} / 300</span>
      </div>
      <p v-if="error" role="alert" class="error">{{ error.message }} <span v-for="hint in error.hints" :key="hint.label">{{ hint.label }}</span></p>
      <div class="cookie-list">
        <table v-if="cookies.length">
          <thead><tr><th>名称 / 值</th><th>域 / 路径</th><th>属性 / 过期时间</th><th>操作</th></tr></thead>
          <tbody><tr v-for="cookie in cookies" :key="cookie.id">
            <td>{{ cookie.name }}<code>{{ reveal ? cookie.value : '••••••' }}</code></td>
            <td>{{ cookie.domain }}<code>{{ cookie.path }}</code></td>
            <td>{{ cookie.hostOnly ? '仅此主机' : '含子域' }} {{ cookie.secure ? 'Secure' : '' }} {{ cookie.httpOnly ? 'HttpOnly' : '' }}<code>{{ cookie.expiresAt ? new Date(cookie.expiresAt * 1000).toLocaleString() : '会话结束' }}</code></td>
            <td><AppButton size="tiny" :disabled="busy" @click="edit(cookie)">编辑</AppButton> <AppButton size="tiny" :disabled="busy" @click="run(async () => { await ipc.workspaceDeleteCookie(cookie.id); if (editing === cookie.id) reset() })">删除</AppButton></td>
          </tr></tbody>
        </table>
        <p v-else>当前会话没有 Cookie。</p>
      </div>
      <form class="editor" @submit.prevent="save">
        <strong>{{ editing ? '编辑 Cookie' : '添加 Cookie' }}</strong>
        <label>来源 URL<AppInput v-model:value="source" placeholder="https://api.example.com/login" :input-props="{ 'aria-label': 'Cookie 来源 URL', spellcheck: false }" /></label>
        <label>Set-Cookie<AppInput v-model:value="header" type="textarea" placeholder="session=value; Path=/; HttpOnly; Secure" :input-props="{ 'aria-label': 'Set-Cookie 内容', spellcheck: false }" /></label>
        <p>可填写 Domain、Path、Expires 或 Max-Age。编辑后按填写的过期属性重新计时。</p>
        <div class="actions"><AppButton attr-type="submit" :disabled="busy || !source || !header">保存 Cookie</AppButton><AppButton :disabled="busy" @click="reset">取消编辑</AppButton></div>
      </form>
    </div>
    <template #footer><AppButton :disabled="busy" @click="$emit('close')">完成</AppButton></template>
  </AppModal>
</template>

<style scoped>
.cookie-manager { display: grid; gap: 12px; color: var(--a-text); font-size: var(--a-fs-sm); }
p { margin: 0; color: var(--a-text-dim); line-height: 1.6; }
.actions { display: flex; align-items: center; gap: 8px; }
.actions span { margin-left: auto; }
.cookie-list { overflow: auto; max-height: 280px; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); }
table { width: 100%; border-collapse: collapse; text-align: left; }
td, th { padding: 8px; border-bottom: 1px solid var(--a-border); vertical-align: top; }
th { position: sticky; top: 0; background: var(--a-bg-2); }
td { max-width: 220px; overflow-wrap: anywhere; }
code { display: block; color: var(--a-text-dim); font-family: var(--a-font-editor); }
.editor, label { display: grid; gap: 8px; }
.error { color: var(--a-error); }
</style>
