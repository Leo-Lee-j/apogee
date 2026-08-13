<script setup lang="ts">
import { computed, ref } from 'vue'
import type { EnvironmentVariable } from '@/ipc'
import { useWorkspaceStore } from '@/stores/workspace'
import {
  AppButton,
  AppCheckbox,
  AppIcon,
  AppInput,
  AppModal,
  AppPopconfirm,
} from '@/components/ui'
import { CubeOutline } from '@vicons/ionicons5'
import { IconAdd, IconCopy, IconDelete } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'

const workspace = useWorkspaceStore()
const open = ref(false)
const saving = ref(false)

const duplicateKeys = computed(() => {
  const counts = new Map<string, number>()
  for (const variable of workspace.activeEnvironment?.variables ?? []) {
    const key = variable.key.trim()
    if (variable.enabled && key) counts.set(key, (counts.get(key) ?? 0) + 1)
  }
  return new Set([...counts].filter(([, count]) => count > 1).map(([key]) => key))
})

const hostError = computed(() => {
  const value = workspace.activeEnvironment?.host.trim() ?? ''
  if (!value) return ''
  try {
    const parsed = new URL(value)
    if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname) {
      return 'Host 必须使用 http:// 或 https://'
    }
    if (parsed.pathname !== '/' || parsed.search || parsed.hash) {
      return 'Host 不能包含路径、查询参数或片段'
    }
    return ''
  } catch {
    return '请输入完整地址，例如 https://api.example.com'
  }
})

const validationMessage = computed(() => {
  const environment = workspace.activeEnvironment
  if (!environment) return ''
  if (!environment.name.trim()) return '环境名称不能为空'
  if (environment.name.trim().length > 120) return '环境名称不能超过 120 个字符'
  if (hostError.value) return hostError.value
  if (duplicateKeys.value.size > 0) return `变量名重复：${[...duplicateKeys.value].join('、')}`
  return ''
})

function displayHost(host: string) {
  if (!host.trim()) return '未配置 Host'
  try {
    return new URL(host).host
  } catch {
    return host
  }
}

function nextEnvironmentName() {
  const names = new Set(workspace.environments.map((environment) => environment.name))
  if (!names.has('新环境')) return '新环境'
  let suffix = 2
  while (names.has(`新环境 ${suffix}`)) suffix += 1
  return `新环境 ${suffix}`
}

function updateEnvironment(delta: { name?: string; host?: string }) {
  const environment = workspace.activeEnvironment
  if (environment) workspace.updateEnvironment(environment.id, delta)
}

function updateVariable(index: number, field: 'key' | 'value', value: string) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  const delta: Partial<EnvironmentVariable> = { [field]: value }
  workspace.updateEnvironmentVariable(environment.id, index, delta)
}

function updateEnabled(index: number, enabled: boolean) {
  const environment = workspace.activeEnvironment
  if (environment) workspace.updateEnvironmentVariable(environment.id, index, { enabled })
}

async function saveActive() {
  if (!workspace.activeEnvironment || !workspace.activeEnvironmentDirty) return true
  if (validationMessage.value) return false
  saving.value = true
  try {
    return await workspace.saveEnvironment()
  } finally {
    saving.value = false
  }
}

async function selectEnvironment(id: string) {
  if (id === workspace.activeEnvironmentId || !(await saveActive())) return
  workspace.selectEnvironment(id)
}

async function createEnvironment() {
  if (!(await saveActive())) return
  await workspace.createEnvironment(nextEnvironmentName())
}

async function duplicateEnvironment() {
  if (!(await saveActive())) return
  await workspace.duplicateEnvironment()
}

function handleVisibility(show: boolean) {
  open.value = show
}

async function finish() {
  if (await saveActive()) open.value = false
}
</script>

<template>
  <div class="environment-controls">
    <AppButton
      class="variables-button"
      size="small"
      :disabled="!workspace.available"
      :aria-expanded="open"
      @click="open = true"
    >
      <template #icon><AppIcon :component="CubeOutline" :size="15" /></template>
      环境配置
      <span v-if="workspace.activeEnvironmentDirty" class="dirty-dot" aria-label="有未保存修改" />
    </AppButton>

    <NaiveThemeScope>
      <AppModal
        :show="open"
        preset="card"
        title="环境配置"
        class="environment-modal"
        :style="{ width: 'min(760px, calc(100vw - 32px))' }"
        :mask-closable="!saving"
        :closable="!saving"
        @update:show="handleVisibility"
      >
        <div class="environment-dialog">
          <aside class="environment-list-pane">
            <div class="pane-heading">
              <span>环境</span>
              <AppButton quaternary circle size="small" title="新建环境" aria-label="新建环境" @click="createEnvironment">
                <template #icon><AppIcon :component="IconAdd" :size="16" /></template>
              </AppButton>
            </div>
            <div class="environment-list">
              <button
                v-for="environment in workspace.environments"
                :key="environment.id"
                type="button"
                class="environment-item"
                :class="{ active: environment.id === workspace.activeEnvironmentId }"
                @click="selectEnvironment(environment.id)"
              >
                <span class="environment-item-name">{{ environment.name }}</span>
                <span class="environment-item-host">{{ displayHost(environment.host) }}</span>
              </button>
              <div v-if="workspace.environments.length === 0" class="list-empty">暂无环境</div>
            </div>
            <div class="environment-list-actions">
              <AppButton
                quaternary
                circle
                size="small"
                title="复制环境"
                aria-label="复制环境"
                :disabled="!workspace.activeEnvironment"
                @click="duplicateEnvironment"
              >
                <template #icon><AppIcon :component="IconCopy" :size="15" /></template>
              </AppButton>
              <AppPopconfirm
                positive-text="删除"
                negative-text="取消"
                @positive-click="workspace.deleteEnvironment()"
              >
                <template #trigger>
                  <AppButton
                    quaternary
                    circle
                    size="small"
                    title="删除环境"
                    aria-label="删除环境"
                    :disabled="!workspace.activeEnvironment"
                  >
                    <template #icon><AppIcon :component="IconDelete" :size="15" /></template>
                  </AppButton>
                </template>
                删除环境“{{ workspace.activeEnvironment?.name }}”及其工作区文件？
              </AppPopconfirm>
            </div>
          </aside>

          <section v-if="workspace.activeEnvironment" class="environment-editor">
            <section class="environment-fields">
              <label class="field field-name">
                <span>环境名称</span>
                <AppInput
                  size="medium"
                  :value="workspace.activeEnvironment.name"
                  maxlength="120"
                  placeholder="例如：开发环境"
                  @update:value="updateEnvironment({ name: $event })"
                />
              </label>
              <label class="field field-host">
                <span>Host</span>
                <AppInput
                  class="host-input"
                  size="medium"
                  :status="hostError ? 'error' : undefined"
                  :value="workspace.activeEnvironment.host"
                  placeholder="https://api.example.com"
                  :input-props="{ spellcheck: false, autocomplete: 'off' }"
                  @update:value="updateEnvironment({ host: $event })"
                />
              </label>
            </section>

            <p v-if="validationMessage" class="validation-message">{{ validationMessage }}</p>

            <section class="variables-section">
              <div class="variables-heading">
                <div>
                  <h3>变量</h3>
                  <span>{{ workspace.activeEnvironment.variables.length }}</span>
                </div>
                <AppButton secondary size="small" @click="workspace.addEnvironmentVariable(workspace.activeEnvironment.id)">
                  <template #icon><AppIcon :component="IconAdd" :size="14" /></template>
                  添加变量
                </AppButton>
              </div>

              <div class="variable-table">
                <div class="variable-row variable-header" aria-hidden="true">
                  <span>启用</span>
                  <span>变量名</span>
                  <span>值</span>
                  <span />
                </div>
                <div
                  v-for="(variable, index) in workspace.activeEnvironment.variables"
                  :key="`${workspace.activeEnvironment.id}-${index}`"
                  class="variable-row"
                >
                  <AppCheckbox
                    :checked="variable.enabled"
                    :aria-label="`启用变量 ${variable.key || '空变量'}`"
                    @update:checked="updateEnabled(index, $event)"
                  />
                  <AppInput
                    class="variable-key"
                    size="medium"
                    :status="variable.enabled && duplicateKeys.has(variable.key.trim()) ? 'error' : undefined"
                    :value="variable.key"
                    placeholder="变量名"
                    :input-props="{ spellcheck: false }"
                    @update:value="updateVariable(index, 'key', $event)"
                  />
                  <AppInput
                    class="variable-value"
                    size="medium"
                    :value="variable.value"
                    placeholder="变量值"
                    :input-props="{ spellcheck: false }"
                    @update:value="updateVariable(index, 'value', $event)"
                  />
                  <AppButton
                    quaternary
                    circle
                    size="small"
                    title="删除变量"
                    aria-label="删除变量"
                    @click="workspace.removeEnvironmentVariable(workspace.activeEnvironment.id, index)"
                  >
                    <template #icon><AppIcon :component="IconDelete" :size="15" /></template>
                  </AppButton>
                </div>
                <div v-if="workspace.activeEnvironment.variables.length === 0" class="variables-empty">
                  暂无变量
                </div>
              </div>
            </section>
          </section>

          <section v-else class="editor-empty">
            <span>选择一个环境进行配置</span>
            <AppButton type="primary" @click="createEnvironment">新建环境</AppButton>
          </section>
        </div>

        <template #footer>
          <div class="modal-footer">
            <span :class="{ dirty: workspace.activeEnvironmentDirty }">
              {{ workspace.activeEnvironmentDirty ? '有未保存修改' : '已保存' }}
            </span>
            <AppButton type="primary" :loading="saving" :disabled="!!validationMessage" @click="finish">
              完成
            </AppButton>
          </div>
        </template>
      </AppModal>
    </NaiveThemeScope>
  </div>
</template>

<style scoped>
.environment-controls {
  display: flex;
  align-items: center;
  flex: 0 0 auto;
  height: 100%;
  padding: 0 8px;
  border-left: 1px solid var(--a-border);
  background: var(--a-bg-2);
}
.variables-button { font-family: var(--a-font-ui); }
.dirty-dot { width: 6px; height: 6px; border-radius: 50%; background: var(--a-warn); }
.environment-dialog {
  display: grid;
  grid-template-columns: 190px minmax(0, 1fr);
  min-height: 400px;
  height: min(58vh, 520px);
  overflow: hidden;
  border: 1px solid var(--a-border);
  border-radius: 6px;
  background: var(--a-bg-1);
}
.environment-list-pane {
  display: grid;
  grid-template-rows: max(44px, var(--a-h-control)) minmax(0, 1fr) max(44px, var(--a-h-control));
  min-width: 0;
  border-right: 1px solid var(--a-border);
  background: var(--a-bg-2);
}
.pane-heading,
.environment-list-actions { display: flex; align-items: center; justify-content: space-between; padding: 0 10px; }
.pane-heading { border-bottom: 1px solid var(--a-border); color: var(--a-text-soft); font-size: var(--a-fs-sm); font-weight: 600; line-height: var(--a-lh-sm); }
.environment-list-actions { justify-content: flex-end; gap: 2px; border-top: 1px solid var(--a-border); }
.environment-list { min-height: 0; overflow: auto; padding: 6px; }
.environment-item {
  width: 100%;
  display: grid;
  gap: 2px;
  min-height: 50px;
  padding: 7px 9px;
  border: 1px solid transparent;
  border-radius: 5px;
  background: transparent;
  color: var(--a-text);
  text-align: left;
  cursor: pointer;
}
.environment-item:hover { background: var(--a-hover); }
.environment-item.active { border-color: color-mix(in srgb, var(--a-primary) 42%, var(--a-border)); background: var(--a-selected); }
.environment-item-name,
.environment-item-host { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.environment-item-name { font-size: var(--a-fs-base); font-weight: 500; line-height: var(--a-lh); }
.environment-item-host { color: var(--a-text-faint); font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-mono); }
.list-empty { padding: 24px 8px; color: var(--a-text-faint); text-align: center; font-size: var(--a-fs-sm); }
.environment-editor { display: flex; flex-direction: column; min-width: 0; min-height: 0; padding: 16px; }
.environment-fields { display: grid; grid-template-columns: 1fr; gap: 10px; }
.field { display: grid; gap: 7px; min-width: 0; color: var(--a-text-soft); font-size: var(--a-fs-sm); }
.field-name { max-width: 300px; }
.field-host { width: 100%; }
.field :deep(.n-input) { min-height: var(--a-h-input-md); }
.field :deep(.n-input-wrapper) { padding-inline: 12px; }
.host-input :deep(.n-input__input-el) { line-height: var(--a-lh) !important; }
.host-input,
.variable-key,
.variable-value { font-family: var(--a-font-ui); }
.validation-message { margin: 8px 0 0; color: var(--a-error); font-size: var(--a-fs-sm); }
.variables-section { display: flex; flex-direction: column; min-height: 0; margin-top: 14px; }
.variables-heading { display: flex; align-items: center; justify-content: space-between; min-height: max(34px, var(--a-h-control-sm)); margin-bottom: 8px; }
.variables-heading > div { display: flex; align-items: baseline; gap: 7px; }
.variables-heading h3 { margin: 0; color: var(--a-text); font-size: var(--a-fs-base); font-weight: 600; }
.variables-heading span { color: var(--a-text-faint); font: var(--a-fs-xs) var(--a-font-mono); }
.variable-table { min-height: 0; overflow: auto; border: 1px solid var(--a-border); border-radius: 6px; background: var(--a-bg-2); }
.variable-row {
  display: grid;
  grid-template-columns: 44px minmax(140px, 0.8fr) minmax(200px, 1.2fr) 36px;
  align-items: center;
  gap: 6px;
  min-height: max(42px, var(--a-h-input-md));
  padding: 4px 6px;
  border-bottom: 1px solid var(--a-border);
}
.variable-row :deep(.n-input) { min-height: var(--a-h-input-md); }
.variable-row :deep(.n-input-wrapper) { padding-inline: 10px; }
.variable-row :deep(.n-input__input-el) { line-height: var(--a-lh) !important; }
.variable-row:last-child { border-bottom: 0; }
.variable-header { position: sticky; top: 0; z-index: 1; min-height: var(--a-h-control-xs); background: var(--a-bg-3); color: var(--a-text-faint); font-size: var(--a-fs-xs); line-height: var(--a-lh-xs); }
.variable-header span:first-child { text-align: center; }
.variables-empty { padding: 42px 12px; color: var(--a-text-faint); text-align: center; font-size: var(--a-fs-sm); }
.editor-empty { display: grid; place-content: center; justify-items: center; gap: 14px; color: var(--a-text-dim); font-size: var(--a-fs-sm); }
.modal-footer { width: 100%; display: flex; align-items: center; justify-content: space-between; }
.modal-footer > span { color: var(--a-text-faint); font-size: var(--a-fs-sm); }
.modal-footer > span.dirty { color: var(--a-warn); }
@media (max-width: 720px) {
  .environment-dialog { grid-template-columns: 160px minmax(0, 1fr); }
  .environment-editor { padding: 12px; }
  .variable-row { grid-template-columns: 36px minmax(100px, 0.8fr) minmax(130px, 1.2fr) 32px; }
}
</style>
