<script setup lang="ts">
import { ref } from 'vue'
import type { EnvironmentVariable } from '@/ipc'
import { useWorkspaceStore } from '@/stores/workspace'

const workspace = useWorkspaceStore()
const open = ref(false)

function selectEnvironment(event: Event) {
  const id = (event.target as HTMLSelectElement).value
  workspace.selectEnvironment(id || null)
  open.value = false
}

function updateEnvironmentName(event: Event) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  workspace.updateEnvironment(environment.id, {
    name: (event.target as HTMLInputElement).value,
  })
}

function updateVariable(index: number, field: 'key' | 'value', event: Event) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  const delta: Partial<EnvironmentVariable> = {
    [field]: (event.target as HTMLInputElement).value,
  }
  workspace.updateEnvironmentVariable(environment.id, index, delta)
}

function updateEnabled(index: number, event: Event) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  workspace.updateEnvironmentVariable(environment.id, index, {
    enabled: (event.target as HTMLInputElement).checked,
  })
}

async function createEnvironment() {
  const created = await workspace.createEnvironment()
  if (created) open.value = true
}
</script>

<template>
  <div class="environment-controls">
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
      class="variables-button"
      type="button"
      :disabled="!workspace.available"
      :aria-expanded="open"
      title="编辑环境变量"
      @click="open = !open"
    >
      变量
    </button>

    <div v-if="open" class="environment-popover" role="dialog" aria-label="环境变量">
      <template v-if="workspace.activeEnvironment">
        <div class="environment-head">
          <input
            class="environment-name"
            :value="workspace.activeEnvironment.name"
            aria-label="环境名称"
            @input="updateEnvironmentName"
          />
          <button type="button" class="link" @click="workspace.saveEnvironment()">保存</button>
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
              @change="updateEnabled(index, $event)"
            />
            <input
              class="environment-key"
              :value="variable.key"
              placeholder="变量名"
              spellcheck="false"
              @input="updateVariable(index, 'key', $event)"
            />
            <input
              class="environment-value"
              :value="variable.value"
              placeholder="变量值"
              spellcheck="false"
              @input="updateVariable(index, 'value', $event)"
            />
            <button
              type="button"
              class="delete"
              aria-label="删除变量"
              @click="workspace.removeEnvironmentVariable(workspace.activeEnvironment.id, index)"
            >
              ×
            </button>
          </div>
        </div>
        <button
          type="button"
          class="link"
          @click="workspace.addEnvironmentVariable(workspace.activeEnvironment.id)"
        >
          + 变量
        </button>
      </template>
      <template v-else>
        <p class="environment-empty">还没有环境。</p>
        <button type="button" class="link" @click="createEnvironment">+ 创建环境</button>
      </template>
    </div>
  </div>
</template>

<style scoped>
.environment-controls {
  position: relative;
  display: flex;
  align-items: center;
  gap: 4px;
  flex: 0 0 auto;
  height: 100%;
  padding: 0 8px;
  border-left: 1px solid var(--a-border);
  background: var(--a-bg-2);
}
.environment-select {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--a-text-faint);
  font-size: var(--a-fs-xs);
}
.environment-select select,
.variables-button {
  height: 24px;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-sm);
  background: var(--a-bg-3);
  color: var(--a-text);
  font: var(--a-fs-xs) var(--a-font-mono);
}
.environment-select select { max-width: 128px; padding: 0 5px; }
.variables-button { padding: 0 7px; color: var(--a-text-dim); cursor: pointer; }
.variables-button:hover:not(:disabled) { color: var(--a-text); background: var(--a-hover); }
.variables-button:disabled { opacity: 0.5; cursor: not-allowed; }
.environment-popover {
  position: absolute;
  z-index: 30;
  top: calc(100% + 5px);
  right: 4px;
  width: min(430px, calc(100vw - 24px));
  padding: 8px;
  background: var(--a-bg-2);
  border: 1px solid var(--a-border-strong);
  border-radius: var(--a-radius-md);
  box-shadow: var(--a-shadow-md);
}
.environment-head { display: flex; align-items: center; gap: 8px; padding-bottom: 7px; border-bottom: 1px solid var(--a-border); }
.environment-name {
  flex: 1;
  min-width: 0;
  height: 26px;
  padding: 0 7px;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-sm);
  background: var(--a-bg-3);
  color: var(--a-text);
  font: 12px var(--a-font-mono);
}
.environment-rows { display: flex; flex-direction: column; gap: 5px; max-height: 260px; padding: 8px 0; overflow: auto; }
.environment-row { display: flex; align-items: center; gap: 4px; min-width: 0; }
.environment-key,
.environment-value {
  min-width: 0;
  height: 25px;
  padding: 0 5px;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-sm);
  background: var(--a-bg-3);
  color: var(--a-text);
  font: 11px var(--a-font-mono);
}
.environment-key { flex: 0 0 118px; }
.environment-value { flex: 1 1 auto; }
.delete { width: 24px; height: 24px; padding: 0; border: 0; background: transparent; color: var(--a-text-faint); cursor: pointer; }
.delete:hover { color: var(--a-error); }
.link { padding: 2px 4px; border: 0; background: transparent; color: var(--a-primary); font: inherit; cursor: pointer; }
.link:hover { text-decoration: underline; }
.environment-empty { margin: 4px 0 8px; color: var(--a-text-dim); font-size: 12px; }
</style>
