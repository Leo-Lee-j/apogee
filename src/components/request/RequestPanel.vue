<script setup lang="ts">
import { ref } from 'vue'

/**
 * 请求编辑区。骨架阶段只落地 Tab 结构与参数表格的骨架。
 * Body 的 JSON 编辑器（CodeMirror）走动态 import，不进首屏。
 */
const TABS = ['Params', 'Headers', 'Body', 'Auth', 'Scripts', 'Settings'] as const
const current = ref<(typeof TABS)[number]>('Params')

interface Row { enabled: boolean; key: string; value: string; desc: string }
const params = ref<Row[]>([
  { enabled: true, key: 'expand', value: 'profile', desc: '展开关联字段' },
  { enabled: false, key: 'page', value: '1', desc: '已禁用' },
])

function addRow() {
  params.value = [...params.value, { enabled: true, key: '', value: '', desc: '' }]
}
</script>

<template>
  <section class="req">
    <nav class="tabs" role="tablist">
      <button
        v-for="t in TABS"
        :key="t"
        type="button"
        role="tab"
        :aria-selected="current === t"
        :class="{ on: current === t }"
        @click="current = t"
      >
        {{ t }}
        <span v-if="t === 'Params'" class="badge">{{ params.filter((p) => p.enabled).length }}</span>
      </button>
      <span class="spacer" />
      <button class="link" type="button">批量编辑</button>
    </nav>

    <div class="body">
      <table v-if="current === 'Params'" class="kv">
        <thead>
          <tr><th class="c" /><th>Key</th><th>Value</th><th>Description</th><th class="c" /></tr>
        </thead>
        <tbody>
          <!--
            勾选框「禁用而非删除」：调试时最常见的动作是临时去掉一个参数
            （02 文档 §4.2）。
          -->
          <tr v-for="(p, i) in params" :key="i" :class="{ off: !p.enabled }">
            <td class="c"><input v-model="p.enabled" type="checkbox" :aria-label="`启用 ${p.key}`" /></td>
            <td><input v-model="p.key" class="cell" spellcheck="false" /></td>
            <td><input v-model="p.value" class="cell" spellcheck="false" /></td>
            <td><input v-model="p.desc" class="cell" /></td>
            <td class="c">
              <button class="del" type="button" aria-label="删除" @click="params = params.filter((_, j) => j !== i)">×</button>
            </td>
          </tr>
          <tr><td class="c" /><td colspan="4"><button class="link" type="button" @click="addRow">+ 新增</button></td></tr>
        </tbody>
      </table>

      <p v-else class="todo">
        {{ current }} —— 骨架阶段占位。Body 的 JSON 编辑器将按 03 文档 §4.1 动态 import，不进首屏 chunk。
      </p>
    </div>
  </section>
</template>

<style scoped>
.req { display: flex; flex-direction: column; min-height: 0; overflow: hidden; }
.tabs {
  height: var(--a-h-toolbar); flex: 0 0 auto;
  display: flex; align-items: center; gap: 2px; padding: 0 6px;
  border-bottom: 1px solid var(--a-border);
}
.tabs button {
  background: transparent; border: none; color: var(--a-text-dim);
  font: inherit; height: 24px; padding: 0 8px; border-radius: 4px; cursor: pointer;
}
.tabs button:hover { background: var(--a-hover); color: var(--a-text); }
.tabs button.on { color: var(--a-text); box-shadow: inset 0 -2px 0 var(--a-primary); border-radius: 0; }
.badge {
  font-size: var(--a-fs-xs); color: var(--a-text-faint);
  background: var(--a-bg-3); border-radius: var(--a-radius-pill); padding: 0 5px; margin-left: 2px;
}
.spacer { flex: 1 1 auto; }
.link { color: var(--a-primary) !important; }
.body { flex: 1 1 auto; overflow: auto; min-height: 0; }
.kv { width: 100%; border-collapse: collapse; font-size: 12px; }
.kv th {
  text-align: left; font-weight: 500; color: var(--a-text-faint);
  padding: 4px 6px; border-bottom: 1px solid var(--a-border); position: sticky; top: 0;
  background: var(--a-bg-1);
}
.kv td { border-bottom: 1px solid var(--a-border); padding: 0; height: var(--a-row); }
.kv tr.off { opacity: 0.45; }
.c { width: 28px; text-align: center; }
.cell {
  width: 100%; height: var(--a-row); background: transparent; border: none;
  color: var(--a-text); font: 12px/1 var(--a-font-mono); padding: 0 6px;
}
.cell:focus { background: var(--a-bg-3); outline: 2px solid var(--a-focus); outline-offset: -2px; }
.del { background: transparent; border: none; color: var(--a-text-faint); cursor: pointer; }
.del:hover { color: var(--a-error); }
.todo { padding: 12px; color: var(--a-text-dim); font-size: 12px; }
</style>
