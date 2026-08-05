<script setup lang="ts">
import { ref, computed, defineAsyncComponent } from 'vue'
import type { AppError } from '@/ipc'
import { useTabsStore } from '@/stores/tabs'

/**
 * 响应区 —— 性能设计的核心战场（02 文档 §4.4）。
 *
 * 首屏只渲染状态行和 Tab 结构。真正的查看器（CodeMirror / 虚拟树）
 * **全部动态 import**，理由见 03 文档 §4.1：CodeMirror 不能进首屏 chunk。
 */

const CodeMirrorView = defineAsyncComponent(() => import('./CodeMirrorView.vue'))
const JsonTreeView = defineAsyncComponent(() => import('./JsonTreeView.vue'))

type View = 'tree' | 'raw' | 'table' | 'preview'
const view = ref<View>('tree')

const tabs = useTabsStore()
const resp = computed(() => tabs.active?.response ?? null)
const requestError = computed<AppError | null>(() => tabs.active?.error ?? null)

/** 分级降级（02 文档 §4.4）。唯一的高亮阈值是 2MB。 */
const tierLabel: Record<string, string> = {
  full: '全功能',
  virtualized: '虚拟滚动',
  noHighlight: '已关闭高亮',
  summaryOnly: '仅摘要',
}

const statusClass = computed(() => {
  const s = resp.value?.status
  if (s == null) return resp.value?.timing ? 'ok' : ''
  if (s < 300) return 'ok'
  if (s < 400) return 'redirect'
  if (s < 500) return 'warn'
  return s < 600 ? 'error' : 'neterr'
})

function fmtBytes(n: number) {
  if (n < 1024) return `${n} B`
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / 1048576).toFixed(2)} MB`
}
</script>

<template>
  <section class="resp">
    <div class="statusline">
      <template v-if="requestError">
        <span class="a-pill status neterr">✕ 请求失败</span>
        <span class="sep">·</span>
        <span>{{ requestError.kind }}</span>
      </template>
      <template v-else-if="resp">
        <!-- 颜色 + 图标 + 文字三通道（02 文档 §7.2）。颜色只是加速识别。 -->
        <span :class="['a-pill', 'status', statusClass]">
          ● {{ resp.status != null ? `${resp.status} ${resp.statusText ?? ''}`.trim() : '···' }}
        </span>
        <span class="sep">·</span>
        <span>{{ resp.timing.totalMs.toFixed(0) }} ms</span>
        <span class="sep">·</span>
        <span>{{ fmtBytes(resp.sizeBytes) }}</span>
        <span class="sep">·</span>
        <span class="tier" :title="`渲染分级：${resp.tier}`">
          {{ tierLabel[resp.tier] }}{{ resp.syntaxHighlight ? '' : '（无高亮）' }}
        </span>
      </template>
      <span v-else class="hint">按 <kbd>Ctrl</kbd><kbd>Enter</kbd> 发送请求</span>

      <span class="spacer" />
      <div v-if="!requestError" class="views" role="tablist">
        <button
          v-for="v in (['tree', 'raw', 'table', 'preview'] as View[])"
          :key="v"
          type="button"
          role="tab"
          :aria-selected="view === v"
          :class="{ on: view === v }"
          @click="view = v"
        >
          {{ { tree: '树', raw: '原文', table: '表格', preview: '预览' }[v] }}
        </button>
      </div>
    </div>

    <div class="pane">
      <div v-if="requestError" class="error-state" role="alert">
        <h3>请求未完成</h3>
        <p class="error-message">{{ requestError.message }}</p>
        <p class="error-kind">错误类型：{{ requestError.kind }}</p>
        <details v-if="requestError.detail" class="error-detail">
          <summary>技术详情</summary>
          <pre>{{ requestError.detail }}</pre>
        </details>
        <ul v-if="requestError.hints.length" class="hints">
          <li v-for="hint in requestError.hints" :key="`${hint.label}-${hint.action ?? ''}`">
            <span>{{ hint.label }}</span>
            <code v-if="hint.action">{{ hint.action }}</code>
          </li>
        </ul>
      </div>
      <template v-else-if="resp">
        <!-- >50MB：不渲染，只给摘要 + 出口（02 文档 §4.4 SummaryOnly 分级） -->
        <div v-if="resp.tier === 'summaryOnly'" class="summary">
          <p class="big">{{ fmtBytes(resp.sizeBytes) }} 响应体</p>
          <p class="dim">超过 50 MB，不在界面内渲染。这是设计行为，不是失败。</p>
          <div class="actions">
            <button type="button">保存到文件</button>
            <button type="button">用外部编辑器打开</button>
            <button type="button" @click="view = 'raw'">流式预览头部</button>
          </div>
        </div>
        <JsonTreeView v-else-if="view === 'tree'" :response-id="resp.id" />
        <CodeMirrorView
          v-else-if="view === 'raw'"
          :response-id="resp.id"
          :total-bytes="resp.sizeBytes"
          :highlight="resp.syntaxHighlight"
        />
        <p v-else class="todo">{{ view }} 视图 —— 骨架阶段占位。</p>
      </template>
      <div v-else class="empty">
        <p>发出请求后，响应会显示在这里。</p>
        <p class="dim">大响应体会按体积自动降级渲染，不会卡住界面。</p>
      </div>
    </div>
  </section>
</template>

<style scoped>
/* 没有 border-top —— 与请求区之间靠 --a-gutter 的间隙分隔（02 文档 §3.2） */
.resp { display: flex; flex-direction: column; min-height: 0; overflow: hidden; }
.statusline {
  height: 30px; flex: 0 0 auto;
  display: flex; align-items: center; gap: 6px; padding: 0 8px;
  background: var(--a-bg-2); border-bottom: 1px solid var(--a-border);
  font-size: 12px; color: var(--a-text-dim);
}
/* 状态徽章。.a-pill 提供形态（见 tokens.css），这里只给语义色。
   网络错误刻意用实色填充而不是换色相：rose 和 red 在半透明 pill 里几乎分不开，
   靠份量差 + 图标区分更可靠（02 文档 §4.4）。 */
.status.ok { color: var(--a-ok); background: var(--a-ok-bg); }
.status.redirect { color: var(--a-redirect); background: var(--a-redirect-bg); }
.status.warn { color: var(--a-warn); background: var(--a-warn-bg); }
.status.error { color: var(--a-error); background: var(--a-error-bg); }
.status.neterr {
  color: var(--a-neterr);
  background: var(--a-neterr-bg);
  border-color: var(--a-neterr-bg);
}
/* 分隔符是纯装饰，不承载信息 */
.sep { color: var(--a-text-ghost); }
.tier { color: var(--a-text-faint); }
.hint kbd {
  font: var(--a-fs-xs) / 1 var(--a-font-mono); background: var(--a-bg-1);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); padding: 2px 4px; margin: 0 1px;
}
.spacer { flex: 1 1 auto; }
.views { display: flex; gap: 2px; }
.views button {
  background: transparent; border: none; color: var(--a-text-dim);
  font: inherit; height: 22px; padding: 0 8px; border-radius: var(--a-radius-md); cursor: pointer;
}
.views button:hover { background: var(--a-hover); }
.views button.on { background: var(--a-selected); color: var(--a-text); }
.pane { flex: 1 1 auto; min-height: 0; overflow: hidden; display: flex; flex-direction: column; }
.error-state, .empty, .summary, .todo { padding: 16px; color: var(--a-text-dim); font-size: 12px; }
.error-state { overflow: auto; }
.error-state h3 { margin: 0 0 8px; color: var(--a-error); font-size: 14px; }
.error-message { margin: 0 0 6px; color: var(--a-text); font-size: 13px; }
.error-kind { margin: 0; color: var(--a-text-faint); font-family: var(--a-font-mono); }
.error-detail { margin-top: 12px; color: var(--a-text-dim); }
.error-detail summary { cursor: pointer; color: var(--a-text); }
.error-detail pre { margin: 6px 0 0; white-space: pre-wrap; font: 11px/1.5 var(--a-font-mono); }
.hints { margin: 14px 0 0; padding-left: 18px; color: var(--a-text-dim); }
.hints li { margin: 5px 0; }
.hints code { margin-left: 6px; color: var(--a-primary); font: 11px var(--a-font-mono); }
.dim { color: var(--a-text-faint); }
.big { font-size: 16px; color: var(--a-text); }
.actions { display: flex; gap: 8px; margin-top: 12px; }
.actions button {
  height: 26px; padding: 0 10px; background: var(--a-bg-3);
  border: 1px solid var(--a-border); border-radius: var(--a-radius-md); color: var(--a-text);
  font: inherit; cursor: pointer;
}
.actions button:hover { background: var(--a-hover); }
</style>
