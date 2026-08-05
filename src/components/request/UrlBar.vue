<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import { useWorkspaceStore } from '@/stores/workspace'
import { useRequest } from '@/stores/request'
import type { EnvironmentVariable } from '@/ipc'

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

const tabs = useTabsStore()
const ui = useUiStore()
const workspace = useWorkspaceStore()
const { sending, send } = useRequest()
const inputEl = ref<HTMLInputElement | null>(null)
const methodOpen = ref(false)
const environmentOpen = ref(false)

const tab = computed(() => tabs.active)

/** 方法选择器的前景色与 pill 底色。两个都取自令牌，OPTIONS 回落到 HEAD 档。 */
const methodVar = computed(() => methodFg(tab.value?.method ?? 'GET'))
const methodBgVar = computed(() => methodBg(tab.value?.method ?? 'GET'))

function methodFg(m: string) {
  return `var(--a-${m.toLowerCase()}, var(--a-head))`
}
function methodBg(m: string) {
  return `var(--a-${m.toLowerCase()}-bg, var(--a-head-bg))`
}

function setMethod(m: string) {
  if (tab.value) tabs.patch(tab.value.id, { method: m, dirty: true })
  methodOpen.value = false
}

function onUrl(e: Event) {
  const v = (e.target as HTMLInputElement).value
  if (tab.value) tabs.patch(tab.value.id, { url: v, dirty: true })
}

function save() {
  if (tab.value) void tabs.save(tab.value.id)
}

function selectEnvironment(event: Event) {
  const id = (event.target as HTMLSelectElement).value
  workspace.selectEnvironment(id || null)
  environmentOpen.value = false
}

function updateEnvironmentName(event: Event) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  workspace.updateEnvironment(environment.id, {
    name: (event.target as HTMLInputElement).value,
  })
}

function updateEnvironmentVariable(
  index: number,
  field: 'key' | 'value',
  event: Event,
) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  const delta: Partial<EnvironmentVariable> = {
    [field]: (event.target as HTMLInputElement).value,
  }
  workspace.updateEnvironmentVariable(environment.id, index, delta)
}

function updateEnvironmentEnabled(index: number, event: Event) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  workspace.updateEnvironmentVariable(environment.id, index, {
    enabled: (event.target as HTMLInputElement).checked,
  })
}

function addEnvironmentVariable() {
  const environment = workspace.activeEnvironment
  if (environment) workspace.addEnvironmentVariable(environment.id)
}

function removeEnvironmentVariable(index: number) {
  const environment = workspace.activeEnvironment
  if (environment) workspace.removeEnvironmentVariable(environment.id, index)
}

async function createEnvironment() {
  const created = await workspace.createEnvironment()
  if (created) environmentOpen.value = true
}

async function saveEnvironment() {
  await workspace.saveEnvironment()
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
    if (!sending.value) send()
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

defineExpose({ focus: () => inputEl.value?.select() })
</script>

<template>
  <div class="urlbar">
    <div class="method-wrap">
      <button
        class="method"
        type="button"
        :style="{ color: methodVar, background: methodBgVar, borderColor: methodVar }"
        aria-haspopup="listbox"
        :aria-expanded="methodOpen"
        @click="methodOpen = !methodOpen"
      >
        {{ tab?.method ?? 'GET' }} <span class="caret">▾</span>
      </button>
      <!--
        自研下拉而不是 n-select：这是首屏热路径。
        02 文档 §7.5 第 5 条把 naive-ui 限定在低频、非热路径场景。
      -->
      <ul v-if="methodOpen" class="menu" role="listbox">
        <li v-for="m in METHODS" :key="m" role="option" :aria-selected="m === tab?.method">
          <button
            type="button"
            :style="{ color: methodFg(m) }"
            @click="setMethod(m)"
          >
            {{ m }}
          </button>
        </li>
      </ul>
    </div>

    <input
      ref="inputEl"
      class="url a-selectable"
      type="text"
      spellcheck="false"
      autocomplete="off"
      placeholder="输入 URL，或粘贴 cURL 命令"
      aria-label="请求地址"
      :value="tab?.url ?? ''"
      @input="onUrl"
      @keydown="onKeydown"
    />

    <div class="environment-wrap">
      <label class="environment-select" title="选择请求变量环境">
        <span>环境</span>
        <select
          :value="workspace.activeEnvironmentId ?? ''"
          aria-label="请求变量环境"
          @change="selectEnvironment"
        >
          <option value="">无环境</option>
          <option v-for="item in workspace.environments" :key="item.id" :value="item.id">
            {{ item.name }}
          </option>
        </select>
      </label>
      <button
        class="environment-button"
        type="button"
        :disabled="!workspace.available"
        :aria-expanded="environmentOpen"
        title="编辑环境变量"
        @click="environmentOpen = !environmentOpen"
      >
        变量
      </button>
      <div v-if="environmentOpen" class="environment-popover" role="dialog" aria-label="环境变量">
        <template v-if="workspace.activeEnvironment">
          <div class="environment-head">
            <input
              class="environment-name"
              :value="workspace.activeEnvironment.name"
              aria-label="环境名称"
              @input="updateEnvironmentName"
            />
            <button type="button" class="link" @click="saveEnvironment">保存</button>
          </div>
          <div class="environment-rows">
            <div
              v-for="(variable, index) in workspace.activeEnvironment.variables"
              :key="`${workspace.activeEnvironment.id}-${index}`"
              class="environment-row"
            >
              <input
                :checked="variable.enabled"
                type="checkbox"
                :aria-label="`启用变量 ${variable.key || '空变量'}`"
                @change="updateEnvironmentEnabled(index, $event)"
              />
              <input
                class="environment-key"
                :value="variable.key"
                placeholder="变量名"
                spellcheck="false"
                @input="updateEnvironmentVariable(index, 'key', $event)"
              />
              <input
                class="environment-value"
                type="text"
                :value="variable.value"
                placeholder="变量值"
                spellcheck="false"
                @input="updateEnvironmentVariable(index, 'value', $event)"
              />
              <button
                type="button"
                class="env-icon delete"
                aria-label="删除变量"
                @click="removeEnvironmentVariable(index)"
              >
                ×
              </button>
            </div>
          </div>
          <div class="environment-actions">
            <button type="button" class="link" @click="addEnvironmentVariable">+ 变量</button>
          </div>
        </template>
        <template v-else>
          <p class="environment-empty">还没有环境。创建一个环境后即可使用 &#123;&#123;variable&#125;&#125;。</p>
          <button type="button" class="link create-environment" @click="createEnvironment">+ 创建环境</button>
        </template>
      </div>
    </div>

    <button
      class="send"
      type="button"
      :disabled="sending"
      @click="send"
    >
      {{ sending ? '等待中…' : '发送' }}
    </button>
    <button class="save" type="button" @click="save">保存</button>
  </div>
</template>

<style scoped>
.urlbar {
  height: var(--a-h-urlbar);
  flex: 0 0 auto;
  position: relative;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 0 8px;
  /* 没有 border-bottom —— URL 栏是「请求头」块的最后一行，
     下边界由块自己的边框画（见 AppBody.vue 的 .head） */
}
.environment-wrap { position: relative; display: flex; align-items: center; gap: 3px; flex: 0 0 auto; }
.environment-select {
  display: inline-flex; align-items: center; gap: 4px; height: 28px;
  color: var(--a-text-faint); font-size: var(--a-fs-xs);
}
.environment-select select {
  max-width: 112px; height: 26px; border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md); background: var(--a-bg-3); color: var(--a-text);
  font: var(--a-fs-xs) var(--a-font-mono); padding: 0 4px;
}
.environment-button {
  height: 26px; padding: 0 6px; border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md); background: var(--a-bg-3); color: var(--a-text-dim);
  font: var(--a-fs-xs) var(--a-font-mono); cursor: pointer;
}
.environment-button:hover:not(:disabled) { color: var(--a-text); background: var(--a-hover); }
.environment-button:disabled { opacity: 0.5; cursor: not-allowed; }
.environment-popover {
  position: absolute; z-index: 30; top: calc(100% + 5px); right: 0;
  width: min(430px, calc(100vw - 24px)); padding: 8px;
  background: var(--a-bg-2); border: 1px solid var(--a-border-strong);
  border-radius: var(--a-radius-md); box-shadow: var(--a-shadow-md);
}
.environment-head, .environment-actions { display: flex; align-items: center; gap: 8px; }
.environment-head { padding-bottom: 7px; border-bottom: 1px solid var(--a-border); }
.environment-name {
  flex: 1; min-width: 0; height: 26px; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md);
  color: var(--a-text); padding: 0 7px; font: 12px var(--a-font-mono);
}
.environment-rows { display: flex; flex-direction: column; gap: 5px; padding: 8px 0; max-height: 260px; overflow: auto; }
.environment-row { display: flex; align-items: center; gap: 4px; min-width: 0; }
.environment-key, .environment-value {
  min-width: 0; height: 25px; background: var(--a-bg-3); border: 1px solid var(--a-border);
  border-radius: var(--a-radius-sm); color: var(--a-text); padding: 0 5px; font: 11px var(--a-font-mono);
}
.environment-key { flex: 0 0 118px; }
.environment-value { flex: 1 1 auto; }
.env-icon {
  width: 24px; height: 24px; padding: 0; border: none; background: transparent;
  color: var(--a-text-faint); cursor: pointer; font: 11px var(--a-font-mono);
}
.env-icon:hover { color: var(--a-text); }
.env-icon.delete:hover { color: var(--a-error); }
.environment-empty { margin: 4px 0 8px; color: var(--a-text-dim); font-size: 12px; }
.create-environment { padding: 0; }

.method-wrap { position: relative; }
/* 方法选择器是全产品最显眼的语义控件，用 Apogee 的「半透明底 + 高亮前景」处理。
   底色与描边由 :style 按当前方法绑定，见 methodBg()。颜色 + 文字双通道，
   两种形态（pill 与裸文字）的对比度都由 scripts/check-contrast.mjs 守着。 */
.method {
  height: 28px; min-width: 84px;
  border: 1px solid transparent;
  border-radius: var(--a-radius-md);
  font: var(--a-fw-semibold) var(--a-fs-sm) / 1 var(--a-font-mono);
  cursor: pointer;
}
.caret { color: currentcolor; opacity: 0.65; font-weight: var(--a-fw-normal); }
.menu {
  position: absolute; top: 32px; left: 0; z-index: 20;
  margin: 0; padding: 4px; list-style: none;
  min-width: 110px;
  background: var(--a-bg-3);
  border: 1px solid var(--a-border-strong);
  border-radius: var(--a-radius-md);
  box-shadow: var(--a-shadow-md);
}
.menu button {
  display: block; width: 100%; text-align: left;
  background: transparent; border: none; border-radius: var(--a-radius-sm);
  padding: 4px 8px; font: var(--a-fw-semibold) var(--a-fs-sm) / 1.6 var(--a-font-mono); cursor: pointer;
}
.menu button:hover { background: var(--a-hover); }
.url {
  flex: 1 1 auto; height: 28px;
  background: var(--a-bg-3);
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  color: var(--a-text);
  font: var(--a-fs-base) / 1 var(--a-font-mono);
  padding: 0 8px;
}
/* 不能写 outline: none（原来是这么写的）—— `.url:focus` 的特异性高于
   tokens.css 里的全局 `:focus-visible`，会把键盘焦点环整个盖掉，违反 PRD §10.4。
   改成 1px 内侧焦点线，保留键盘可见性但不增加输入框视觉重量。 */
.url:focus-visible {
  border-color: var(--a-focus);
  outline: 1px solid var(--a-focus);
  outline-offset: -1px;
}
.send, .save {
  height: 28px; padding: 0 14px; border-radius: var(--a-radius-md); cursor: pointer;
  font: inherit;
}
/* 唯一使用渐变的元素（Apogee 规范里的「Launch 按钮」）。
   前景用 --a-on-primary：暗主题下是深色，因为白字压在 #4F80FF 上只有 3.58:1、
   压在渐变末端 #38BDF8 上只有 2.14:1 —— 那会让按钮右半边的字不可读。 */
.send {
  background: var(--a-primary-gradient);
  border: 1px solid transparent;
  color: var(--a-on-primary);
  font-weight: var(--a-fw-semibold);
}
.send:hover { background: var(--a-primary-hover); }
.send:active { background: var(--a-primary-pressed); }
.save { background: var(--a-bg-3); border: 1px solid var(--a-border); color: var(--a-text); }
.save:hover { background: var(--a-hover); }
</style>
