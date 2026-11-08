<script setup lang="ts">
import { useOAuth2TokenActions } from '@/features/oauth2TokenOperation'
import { computed, ref, watch } from 'vue'
import { AppAlert, AppButton, AppIcon, AppInputNumber, AppSelect } from '@/components/ui'
import { ipc, normalizeError, type AuthConfig, type RequestSettings, type WorkspaceContainer } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import AuthEditor from '@/components/request/AuthEditor.vue'
import { authSummary } from '@/features/requestAuth'
import WorkspaceContent from '@/components/layout/WorkspaceContent.vue'
import WorkspaceToolbar from '@/components/layout/WorkspaceToolbar.vue'
import WorkspaceForm from '@/components/layout/WorkspaceForm.vue'
import WorkspaceField from '@/components/layout/WorkspaceField.vue'
import { NBreadcrumb, NBreadcrumbItem, NTimeline, NTimelineItem } from 'naive-ui'
import { IconSave } from '@/components/ui/icons'

const props = defineProps<{ nodeId: string | null }>()
const tokenFetcher = useOAuth2TokenActions()
const tabs = useTabsStore()
const workspace = useWorkspaceStore()
const selectedNodeId = ref(props.nodeId)
const reloadSequence = ref(0)
const loading = ref(false)
const saving = ref(false)
const error = ref('')
const container = ref<WorkspaceContainer | null>(null)
const persistedAuth = ref<AuthConfig | undefined>()
const chainConfigs = ref(new Map<string, WorkspaceContainer>())

const currentNode = computed(() => workspace.nodes?.find((node) => node.id === selectedNodeId.value) ?? null)
const anchorNode = computed(() => workspace.nodes?.find((node) => node.id === props.nodeId) ?? null)
const chain = computed(() => {
  const nodes = workspace.nodes ?? []
  const byId = new Map(nodes.map((node) => [node.id, node]))
  const result = []
  let cursor = anchorNode.value
  while (cursor) {
    if (cursor.isFolder) result.push(cursor)
    cursor = cursor.parentId ? byId.get(cursor.parentId) ?? null : null
  }
  return result.reverse()
})

watch(() => props.nodeId, (id) => { selectedNodeId.value = id }, { immediate: true })
watch([() => selectedNodeId.value, () => workspace.nodes, () => reloadSequence.value,
  () => workspace.workspaceId, () => workspace.sessionId] as const, async ([id], _previous, onCleanup) => {
  let current = true
  onCleanup(() => { current = false })
  container.value = null
  persistedAuth.value = undefined
  chainConfigs.value = new Map()
  error.value = ''
  loading.value = false
  if (!id) return
  loading.value = true
  try {
    const loaded = await Promise.all(chain.value.map(async (node) => [node.id, await ipc.workspaceReadContainer(node.id)] as const))
    if (!current) return
    chainConfigs.value = new Map(loaded)
    container.value = chainConfigs.value.get(id) ?? null
    persistedAuth.value = container.value?.auth
  }
  catch { if (current) error.value = '读取配置失败，请检查工作区后重新加载。' }
  finally { if (current) loading.value = false }
}, { immediate: true })

function selectLayer(id: string) {
  const node = workspace.nodes?.find((item) => item.id === id)
  if (!node?.isFolder) return
  selectedNodeId.value = id
  if (tabs.activeId) tabs.patch(tabs.activeId, { title: `配置 · ${node.name}` })
}

const auth = computed<AuthConfig>(() => container.value?.auth ?? { type: 'inherit' })
const settings = computed<RequestSettings>(() => container.value?.settings ?? {})
const selectedLayerIndex = computed(() => chain.value.findIndex((layer) => layer.id === selectedNodeId.value))
const hasParentLayer = computed(() => selectedLayerIndex.value > 0)
const inheritedAuth = computed<AuthConfig>(() => {
  let resolved: AuthConfig = { type: 'none' }
  for (const layer of chain.value.slice(0, selectedLayerIndex.value)) {
    const value = layerConfig(layer.id)?.auth
    if (value && value.type !== 'inherit') resolved = value
  }
  return resolved
})
function inheritedBooleanSetting(field: 'followRedirects' | 'verifyTls') {
  for (const layer of [...chain.value.slice(0, selectedLayerIndex.value)].reverse()) {
    const value = layerConfig(layer.id)?.settings[field]
    if (typeof value === 'boolean') return value
  }
  return true
}
const inheritedFollowRedirects = computed(() => inheritedBooleanSetting('followRedirects'))
const inheritedVerifyTls = computed(() => inheritedBooleanSetting('verifyTls'))
function inheritedNumberSetting(field: 'timeoutMs' | 'maxRedirects') {
  for (const layer of [...chain.value.slice(0, selectedLayerIndex.value)].reverse()) {
    const value = layerConfig(layer.id)?.settings[field]
    if (typeof value === 'number') return value
  }
  return undefined
}
const inheritedTimeoutMs = computed(() => inheritedNumberSetting('timeoutMs'))
const inheritedMaxRedirects = computed(() => inheritedNumberSetting('maxRedirects'))
function numberSettingPlaceholder(value: number | undefined, defaultValue: number, unit: string) {
  if (!hasParentLayer.value) return `默认值（${defaultValue} ${unit}）`
  return value === undefined
    ? `使用默认值（${defaultValue} ${unit}）`
    : `继承上级（${value} ${unit}）`
}
const timeoutPlaceholder = computed(() => numberSettingPlaceholder(inheritedTimeoutMs.value, 30_000, 'ms'))
const maxRedirectsPlaceholder = computed(() => numberSettingPlaceholder(inheritedMaxRedirects.value, 10, '次'))
const followRedirectOptions = computed(() => [
  ...(hasParentLayer.value ? [{ label: `继承上级（${inheritedFollowRedirects.value ? '开启' : '关闭'}）`, value: 'inherit' }] : []),
  { label: '开启', value: 'true' },
  { label: '关闭', value: 'false' },
])
const verifyTlsOptions = computed(() => [
  ...(hasParentLayer.value ? [{ label: `继承上级（${inheritedVerifyTls.value ? '校验' : '忽略'}）`, value: 'inherit' }] : []),
  { label: '校验', value: 'true' },
  { label: '忽略', value: 'false' },
])
function booleanSelectValue(value: boolean | null | undefined, fallback: boolean) {
  if (typeof value === 'boolean') return String(value)
  return hasParentLayer.value ? 'inherit' : String(fallback)
}
function patchAuth(next: AuthConfig) { if (container.value) container.value = { ...container.value, auth: next } }
function authGuard(id: string) { return () => !saving.value && selectedNodeId.value === id && container.value?.id === id }
function patchSettings(delta: Partial<RequestSettings>) { if (container.value) container.value = { ...container.value, settings: { ...settings.value, ...delta } } }
function patchBooleanSetting(field: 'followRedirects' | 'verifyTls', value: string | number | null) {
  patchSettings({ [field]: value === 'inherit' ? undefined : value === 'true' } as Partial<RequestSettings>)
}
function layerConfig(id: string) { return chainConfigs.value.get(id) }
function settingsSummary(value: RequestSettings | undefined) {
  if (!value) return '正在读取...'
  const parts = []
  if (value.timeoutMs !== undefined) parts.push(`超时 ${value.timeoutMs}ms`)
  if (value.followRedirects !== undefined) parts.push(value.followRedirects ? '跟随重定向' : '不跟随重定向')
  if (value.verifyTls !== undefined) parts.push(value.verifyTls ? '校验 TLS' : '忽略 TLS')
  return parts.length ? parts.join(' · ') : '未设置请求参数'
}
async function save() {
  if (!container.value) return
  const target = container.value
  const workspaceId = workspace.workspaceId, sessionId = workspace.sessionId
  const current = () => selectedNodeId.value === target.id && container.value?.id === target.id
    && workspace.workspaceId === workspaceId && workspace.sessionId === sessionId
  saving.value = true; error.value = ''
  try {
    const saved = await ipc.workspaceSaveContainer(target, persistedAuth.value)
    if (!current()) return
    container.value = saved
    persistedAuth.value = saved.auth
    await workspace.load()
  } catch (cause) {
    if (current()) error.value = normalizeError(cause).message === 'AUTH_CONFIG_STALE' ? '认证来源已更新，请重新加载配置后再保存；未覆盖新凭据。' : '保存配置失败，请检查工作区或重新加载配置。'
  }
  finally { saving.value = false }
}
</script>

<template>
  <main class="container-config">
    <WorkspaceToolbar class="config-toolbar">
      <div class="toolbar-title"><strong>{{ currentNode?.name ?? '容器配置' }}</strong><span>继承链配置</span></div>
      <AppButton quaternary :disabled="loading || saving" title="放弃本页未保存修改并重新读取磁盘配置" @click="reloadSequence++">重新加载</AppButton>
      <AppButton quaternary type="primary" :loading="saving" :disabled="loading || !container" @click="save"><template #icon><AppIcon :component="IconSave" :size="15" /></template>保存</AppButton>
    </WorkspaceToolbar>
    <div class="config-body">
      <AppAlert v-if="error" type="error" :show-icon="false">{{ error }}</AppAlert>
      <div v-if="loading" class="empty">正在读取配置...</div>
      <WorkspaceContent v-else-if="container" split layout="side-by-side" :initial-ratio="0.34" :panel-padding="8">
      <template #primary>
      <section class="chain" aria-label="配置继承链"><header class="section-head"><NBreadcrumb><NBreadcrumbItem v-for="layer in chain" :key="layer.id"><button type="button" class="breadcrumb-node" :class="{ current: layer.id === selectedNodeId }" @click="selectLayer(layer.id)">{{ layer.name }}</button></NBreadcrumbItem></NBreadcrumb></header><NTimeline class="timeline"><NTimelineItem v-for="layer in chain" :key="layer.id" :type="layer.id === selectedNodeId ? 'success' : 'default'"><button type="button" class="timeline-node" :class="{ current: layer.id === selectedNodeId }" @click="selectLayer(layer.id)"><strong>{{ layer.kind === 'collection' ? '集合' : '文件夹' }} · {{ layer.name }}</strong><span>{{ authSummary(layerConfig(layer.id)?.auth ?? { type: 'inherit' }) }} · {{ settingsSummary(layerConfig(layer.id)?.settings) }}</span></button></NTimelineItem></NTimeline></section>
      </template>
      <template #secondary><div class="config-editor">
      <section class="config-block editor">
        <header class="section-head"><strong>鉴权</strong><span>当前层配置</span></header>
        <AuthEditor allow-auto-refresh :key="container.id" :value="auth" :allow-inherit="hasParentLayer"
          :inherit-label="`继承上级（${authSummary(inheritedAuth)}）`" :can-edit="authGuard(container.id)" :fetch-token="tokenFetcher(container.id, settings, authGuard(container.id), container.id, false, undefined, () => persistedAuth)" @update:value="patchAuth" @token-updated="persistedAuth = $event" />
      </section>
      <section class="config-block editor"><header class="section-head"><strong>请求设置</strong></header><WorkspaceForm class="settings-grid" :columns="2"><WorkspaceField label="超时（ms）"><AppInputNumber :value="settings.timeoutMs ?? null" :min="1" clearable :placeholder="timeoutPlaceholder" @update:value="patchSettings({ timeoutMs: $event ?? undefined })" /></WorkspaceField><WorkspaceField label="最大重定向"><AppInputNumber :value="settings.maxRedirects ?? null" :min="0" :max="100" clearable :placeholder="maxRedirectsPlaceholder" @update:value="patchSettings({ maxRedirects: $event ?? undefined })" /></WorkspaceField><WorkspaceField label="跟随重定向"><AppSelect :value="booleanSelectValue(settings.followRedirects, inheritedFollowRedirects)" :options="followRedirectOptions" @update:value="patchBooleanSetting('followRedirects', $event)" /></WorkspaceField><WorkspaceField label="校验 TLS"><AppSelect :value="booleanSelectValue(settings.verifyTls, inheritedVerifyTls)" :options="verifyTlsOptions" @update:value="patchBooleanSetting('verifyTls', $event)" /></WorkspaceField></WorkspaceForm></section>
      </div></template>
      </WorkspaceContent>
      <div v-else class="empty">找不到该容器配置。</div>
    </div>
  </main>
</template>

<style scoped>
.container-config { display: flex; flex-direction: column; min-height: 0; overflow: hidden; }
.config-toolbar { margin: 0 var(--a-gutter); border-radius: var(--a-radius-md); }
.toolbar-title { display: flex; align-items: baseline; gap: 10px; min-width: 0; } .toolbar-title strong { font-size: var(--a-fs-base); } .toolbar-title span { color: var(--a-text-faint); font-size: var(--a-fs-xs); }
.config-body { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; overflow: hidden; margin: var(--a-gutter); border-radius: var(--a-radius-md); background: var(--a-bg-1); }
.chain { height: 100%; overflow: auto; } .editor { margin: 0 0 8px; }
.config-editor { height: 100%; overflow: auto; }
.chain-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; min-height: calc(var(--a-h-control) + 10px); padding: 8px 10px; border-bottom: 1px solid var(--a-border); }
.chain-row strong { display: block; font-size: var(--a-fs-base); } .chain-row span { font-size: var(--a-fs-sm); color: var(--a-text-faint); }
.timeline { margin-top: 14px; } .timeline-node { display: grid; gap: 4px; width: 100%; padding: 5px 8px; border: 0; border-radius: 4px; background: transparent; color: var(--a-text); text-align: left; cursor: pointer; } .timeline-node:hover, .timeline-node.current { background: var(--a-selected); } .timeline-node span { color: var(--a-text-faint); font-size: var(--a-fs-xs); }
.breadcrumb-node { border: 0; padding: 2px 4px; border-radius: 3px; background: transparent; color: var(--a-text-dim); cursor: pointer; font: inherit; } .breadcrumb-node:hover, .breadcrumb-node.current { color: var(--a-primary); background: var(--a-selected); }
.current-mark { color: var(--a-accent) !important; } .editor { overflow: hidden; }
.editor-form, .settings-grid { align-content: start; }
.editor-form .hint { margin: 0; }
.empty { margin: 24px 8px; color: var(--a-text-dim); font-size: var(--a-fs-sm); }
@media (max-width: 680px) { .settings-grid, .editor-form { grid-template-columns: 1fr; } .editor-form .field:first-child, .editor-form .hint { grid-column: 1; } }
</style>
