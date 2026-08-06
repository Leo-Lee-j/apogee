<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { h } from 'vue'
import type { SelectOption } from 'naive-ui'
import { useTabsStore } from '@/stores/tabs'
import { useUiStore } from '@/stores/ui'
import { useRequest } from '@/stores/request'
import { AppSelect } from '@/components/ui'
import { AppButton } from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'

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
const { sending, send } = useRequest()
const inputEl = ref<HTMLInputElement | null>(null)

const tab = computed(() => tabs.active)

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

function onUrl(e: Event) {
  const v = (e.target as HTMLInputElement).value
  if (tab.value) tabs.patch(tab.value.id, { url: v, dirty: true })
}

function save() {
  if (tab.value) void tabs.save(tab.value.id)
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
  <NaiveThemeScope>
    <AppSelect
      class="method-select"
      :value="tab?.method ?? 'GET'"
      :options="METHODS.map((value) => ({ label: value, value }))"
      :render-label="renderMethodLabel"
      size="small"
      @update:value="setMethod(String($event))"
    />

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

    <AppButton
      class="send"
      :disabled="sending"
      @click="send"
    >
      {{ sending ? '等待中…' : '发送' }}
    </AppButton>
    <AppButton class="save" @click="save">保存</AppButton>
  </NaiveThemeScope>
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
.method-wrap { position: relative; }
.method-select { width: 116px; flex: 0 0 116px; }
.method-select :deep(.n-base-selection) { font-family: var(--a-font-mono); font-weight: var(--a-fw-semibold); }
.method-select :deep(.n-base-selection-label) { justify-content: center; }
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
