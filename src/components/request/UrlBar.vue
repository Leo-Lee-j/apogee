<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { h } from 'vue'
import {
  NRadio,
  type DropdownOption,
  type InputInst,
  type SelectOption,
} from 'naive-ui'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import { useRequest } from '@/stores/request'
import { useWorkspaceStore } from '@/stores/workspace'
import { AppButton, AppDropdown, AppIcon, AppSelect } from '@/components/ui'
import { IconLink, IconSave, IconSend } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { saveCurrentRequest } from '@/features/saveRequest'
import SaveToPicker from './SaveToPicker.vue'
import VariableInput from './VariableInput.vue'

/**
 * URL 栏，40px —— 全产品最重要的一行（02 文档 §4.1）。
 *
 * 骨架阶段用原生 input。正式实现要换成轻量代码编辑器，因为需要：
 *   - 变量高亮（绿=已解析 / 红=未定义）
 *   - 输入 `{{` 弹变量补全
 *   - Path 参数 `:id` 识别并联动 Params 面板
 *
 * 关键性能约束（03 文档 §4.6）：输入不触发任何跨 IPC 调用。
 * 变量解析结果本地缓存，只在环境变更时失效。
 */

const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'] as const
const NO_ENVIRONMENT_KEY = '__no_environment__'

const tabs = useTabsStore()
const ui = useUiStore()
const workspace = useWorkspaceStore()
const { sending, send } = useRequest()
const inputEl = ref<InputInst | null>(null)
const urlFocused = ref(false)

const tab = computed(() => tabs.active)
const environmentOptions = computed(() => [
  {
    label: '无环境',
    key: NO_ENVIRONMENT_KEY,
    host: '不使用环境',
    environmentName: '无环境',
  },
  ...workspace.environments.map((environment) => {
    const host = displayHost(environment.host)
    return {
      label: environment.name,
      key: environment.id,
      host: host || '未配置 Host',
      environmentName: environment.name,
    }
  }),
])
const activeHost = computed(() => {
  const environment = workspace.activeEnvironment
  return environment ? displayHost(environment.host) : ''
})

function displayHost(host: string) {
  const value = host.trim()
  if (!value) return ''
  try {
    return new URL(value).host
  } catch {
    return value
  }
}

function selectEnvironment(key: string | number) {
  workspace.selectEnvironment(key === NO_ENVIRONMENT_KEY ? null : String(key))
}

function renderEnvironmentLabel(option: DropdownOption) {
  const selectedKey = workspace.activeEnvironmentId ?? NO_ENVIRONMENT_KEY
  return h('div', {
    style: {
      display: 'grid',
      gridTemplateColumns: '18px minmax(150px, max-content) max-content',
      alignItems: 'center',
      columnGap: '5px',
      width: 'max-content',
      minWidth: '320px',
      maxWidth: 'min(620px, calc(100vw - 48px))',
    },
  }, [
    h(NRadio, {
      checked: option.key === selectedKey,
      focusable: false,
      style: { pointerEvents: 'none' },
    }),
    h('span', {
      style: {
        overflow: 'hidden',
        maxWidth: '360px',
        color: 'var(--a-text)',
        fontFamily: 'var(--a-font-mono)',
        textOverflow: 'ellipsis',
        whiteSpace: 'nowrap',
      },
    }, String(option.host ?? '未配置 Host')),
    h('span', {
      style: {
        overflow: 'hidden',
        justifySelf: 'end',
        maxWidth: '160px',
        marginLeft: '20px',
        color: 'var(--a-text-dim)',
        fontSize: 'var(--a-fs-xs)',
        textAlign: 'right',
        textOverflow: 'ellipsis',
        whiteSpace: 'nowrap',
      },
    }, String(option.environmentName ?? '')),
  ])
}

function setMethod(m: string) {
  if (tab.value) tabs.patch(tab.value.id, { method: m, dirty: true })
}

function methodFg(m: string) {
  return `var(--a-${m.toLowerCase()}, var(--a-head))`
}

function renderMethodLabel(option: SelectOption, selected: boolean) {
  return h(
    'span',
    {
      style: {
        display: 'block',
        width: '100%',
        color: methodFg(String(option.value)),
        fontFamily: 'var(--a-font-mono)',
        fontWeight: 'var(--a-fw-semibold)',
        textAlign: selected ? 'center' : 'left',
      },
    },
    String(option.label),
  )
}

function onUrl(value: string) {
  if (tab.value) tabs.patch(tab.value.id, { url: value, dirty: true })
}

/** 保存由 saveCurrentRequest 统一处理反馈；草稿会打开「保存到」选择器。 */
function save() {
  void saveCurrentRequest()
}

function triggerSend() {
  if (!sending.value) void send()
}

/** URL 中出现 :id / {id} 时自动创建对应的路径参数行。 */
watch(
  () => tab.value?.url,
  (url) => {
    const current = tab.value
    if (current && url !== undefined) tabs.syncPathParams(current.id, url)
  },
  { immediate: true },
)

/** Ctrl+Enter 发送（02 文档 §4.4 ResponsePanel 的快捷键提示）。 */
function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    triggerSend()
  }
}

/** 分阶段启动的收尾动作：首屏就绪后聚焦 URL 栏，让用户能立刻打字（03 文档 §4.1）。 */
onMounted(() => inputEl.value?.focus())

/**
 * 别处新建了请求 → 焦点交到这里（02 文档 §5.1「选完立刻进主界面并聚焦 URL 栏」）。
 *
 * 只 `select()` 不够：`select()` 只管选中文本，聚焦与否各引擎行为不一致。
 * 新建出来的 URL 是空的，选中什么都没有，真正需要的是 focus。
 */
watch(
  () => ui.urlFocusSeq,
  () => {
    inputEl.value?.focus()
    inputEl.value?.select()
  },
)

defineExpose({
  focus: () => inputEl.value?.select(),
  send: triggerSend,
})
</script>

<template>
  <div class="urlbar">
  <NaiveThemeScope>
    <div class="request-input-group">
      <div class="request-editor" :class="{ focused: urlFocused }">
        <AppSelect
          :class="['method-select', `method-${String(tab?.method ?? 'GET').toLowerCase()}`]"
          :value="tab?.method ?? 'GET'"
          :options="METHODS.map((value) => ({ label: value, value }))"
          :render-label="renderMethodLabel"
          :consistent-menu-width="false"
          size="medium"
          @update:value="setMethod(String($event))"
        />

        <div class="environment-prefix">
          <AppDropdown
            trigger="click"
            placement="bottom-start"
            :options="environmentOptions"
            :render-label="renderEnvironmentLabel"
            :disabled="!workspace.available"
            @select="selectEnvironment"
          >
            <button
              class="environment-tag"
              :class="{ active: workspace.activeEnvironment }"
              type="button"
              tabindex="-1"
              :aria-label="workspace.activeEnvironment ? `当前环境 ${workspace.activeEnvironment.name}，点击切换` : '未选择环境，点击切换'"
              @pointerdown.stop
              @mousedown.stop.prevent
              @click.stop
            >
              <AppIcon :component="IconLink" :size="14" />
              <span v-if="workspace.activeEnvironment" class="environment-host">
                {{ activeHost || '未配置 Host' }}
              </span>
            </button>
          </AppDropdown>
        </div>
        <VariableInput
          ref="inputEl"
          class="url a-selectable"
          size="medium"
          :bordered="false"
          placeholder="输入 URL，或粘贴 cURL 命令"
          :value="tab?.url ?? ''"
          :input-props="{ spellcheck: false, autocomplete: 'off', 'aria-label': '请求地址' }"
          @update:value="onUrl"
          @keydown="onKeydown"
          @focus="urlFocused = true"
          @blur="urlFocused = false"
        />
      </div>
    </div>

    <AppButton
      class="send"
      type="primary"
      size="medium"
      :loading="sending"
      :disabled="sending"
      @click="triggerSend"
    >
      <template #icon><AppIcon :component="IconSend" :size="16" /></template>
      发送
    </AppButton>
    <AppButton class="save" secondary size="medium" @click="save">
      <template #icon><AppIcon :component="IconSave" :size="16" /></template>
      保存
    </AppButton>

    <!-- 草稿保存：位置选择器（保存按钮与 Ctrl+S 共用，状态在 ui store） -->
    <SaveToPicker v-model:show="ui.saveAsOpen" />
  </NaiveThemeScope>
  </div>
</template>

<style scoped>
.urlbar {
  --url-control-height: 36px;
  --url-inner-height: 30px;
  --url-radius: 12px;
  --url-inner-radius: 8px;
  height: 52px;
  flex: 0 0 auto;
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 10px;
  /* 没有 border-bottom —— URL 栏是「请求头」块的最后一行，
     下边界由块自己的边框画（见 AppBody.vue 的 .head） */
}
.request-input-group {
  flex: 1 1 auto;
  min-width: 0;
  height: var(--url-control-height);
}
.request-editor {
  box-sizing: border-box;
  display: flex;
  align-items: center;
  min-width: 0;
  height: 100%;
  overflow: hidden;
  border: 1px solid var(--a-border);
  border-radius: var(--url-radius);
  background: var(--a-bg-2);
  padding: 3px 4px;
  gap: 4px;
  transition: border-color 150ms ease, box-shadow 150ms ease;
}
.request-editor.focused {
  z-index: 1;
  border-color: var(--a-primary);
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--a-primary) 18%, transparent);
}
.method-select { width: 76px; height: var(--url-inner-height); flex: 0 0 auto; }
.method-select :deep(.n-base-selection) {
  display: flex;
  align-items: center;
  height: var(--url-inner-height);
  min-height: var(--url-inner-height);
  border: 0;
  border-radius: var(--url-radius);
  --n-border: transparent !important;
  --n-border-hover: transparent !important;
  --n-border-focus: transparent !important;
  --n-box-shadow-hover: none !important;
  --n-box-shadow-focus: none !important;
  --n-box-shadow-active: none !important;
  background: color-mix(in srgb, var(--a-primary) 14%, var(--a-bg-2));
  font-family: var(--a-font-mono);
  font-weight: var(--a-fw-semibold);
}
.method-select :deep(.n-base-selection-label) {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  border-radius: inherit;
  background: color-mix(in srgb, var(--a-primary) 14%, var(--a-bg-2));
  padding-right: 18px;
}
.method-select :deep(.n-base-selection:hover),
.method-select :deep(.n-base-selection:focus),
.method-select :deep(.n-base-selection--active),
.method-select:focus-within :deep(.n-base-selection) {
  border-color: transparent !important;
  box-shadow: none !important;
  outline: none !important;
}
.method-select.method-get :deep(.n-base-selection),
.method-select.method-get :deep(.n-base-selection-label) { background: var(--a-get-bg); }
.method-select.method-post :deep(.n-base-selection),
.method-select.method-post :deep(.n-base-selection-label) { background: var(--a-post-bg); }
.method-select.method-put :deep(.n-base-selection),
.method-select.method-put :deep(.n-base-selection-label) { background: var(--a-put-bg); }
.method-select.method-patch :deep(.n-base-selection),
.method-select.method-patch :deep(.n-base-selection-label) { background: var(--a-patch-bg); }
.method-select.method-delete :deep(.n-base-selection),
.method-select.method-delete :deep(.n-base-selection-label) { background: var(--a-delete-bg); }
.method-select :deep(.n-base-selection-label__render-label) {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100%;
  line-height: 1;
  overflow: hidden;
}
.method-select :deep(.n-base-selection__arrow),
.method-select :deep(.n-base-selection__arrow .n-icon),
.method-select :deep(.n-base-selection__arrow svg) { right: 5px; width: 16px; color: currentColor !important; fill: currentColor; opacity: 1 !important; }
.method-select.method-get :deep(.n-base-selection__arrow) { color: var(--a-get); }
.method-select.method-post :deep(.n-base-selection__arrow) { color: var(--a-post); }
.method-select.method-put :deep(.n-base-selection__arrow) { color: var(--a-put); }
.method-select.method-patch :deep(.n-base-selection__arrow) { color: var(--a-patch); }
.method-select.method-delete :deep(.n-base-selection__arrow) { color: var(--a-delete); }
.method-select.method-get :deep(.n-base-selection), .method-select.method-get :deep(.n-base-suffix__arrow) { --n-arrow-color: var(--a-get) !important; }
.method-select.method-post :deep(.n-base-selection), .method-select.method-post :deep(.n-base-suffix__arrow) { --n-arrow-color: var(--a-post) !important; }
.method-select.method-put :deep(.n-base-selection), .method-select.method-put :deep(.n-base-suffix__arrow) { --n-arrow-color: var(--a-put) !important; }
.method-select.method-patch :deep(.n-base-selection), .method-select.method-patch :deep(.n-base-suffix__arrow) { --n-arrow-color: var(--a-patch) !important; }
.method-select.method-delete :deep(.n-base-selection), .method-select.method-delete :deep(.n-base-suffix__arrow) { --n-arrow-color: var(--a-delete) !important; }
.method-select :deep(.n-base-suffix) { right: 4px; width: 18px; justify-content: center; }
.environment-prefix {
  display: flex;
  align-items: center;
  align-self: stretch;
  flex: 0 0 auto;
  padding: 0;
}
.environment-tag {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  max-width: min(260px, 28vw);
  height: var(--url-inner-height);
  padding: 0 7px;
  border: 0;
  border-radius: var(--url-inner-radius);
  background: color-mix(in srgb, var(--a-bg-2) 82%, var(--a-bg-3));
  color: var(--a-text-dim);
  cursor: pointer;
}
.environment-tag:hover { color: var(--a-text); }
.environment-tag.active {
  background: color-mix(in srgb, var(--a-primary) 9%, var(--a-bg-2));
  color: var(--a-primary-hover);
}
.environment-host {
  overflow: hidden;
  color: var(--a-text-soft);
  font-family: var(--a-font-mono);
  font-size: var(--a-fs-xs);
  text-overflow: ellipsis;
  white-space: nowrap;
}
.url { flex: 1 1 auto; min-width: 0; height: 100%; background: transparent; }
.url :deep(.n-input-wrapper) { height: 100%; align-items: center; }
.url :deep(.n-input__input) { height: 100%; display: flex; align-items: center; }
.url :deep(.n-input__input-el) {
  height: 100%;
  padding-block: 0;
  line-height: 20px;
  font-family: var(--a-font-mono);
  font-size: var(--a-fs-base);
}
.send, .save {
  min-width: 54px;
  height: var(--url-control-height);
  min-height: var(--url-control-height);
  border-radius: var(--url-radius);
}
.send { font-weight: var(--a-fw-semibold); }
</style>
