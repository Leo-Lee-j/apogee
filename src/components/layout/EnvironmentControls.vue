<script setup lang="ts">
import { computed, ref } from 'vue'
import type { EnvironmentVariable } from '@/ipc'
import { useWorkspaceStore } from '@/stores/workspace'
import { AppButton, AppCheckbox, AppIcon, AppInput, AppPopover, AppSelect } from '@/components/ui'
import { IconAdd, IconDelete } from '@/components/ui/icons'

const workspace = useWorkspaceStore()
const open = ref(false)
const environmentOptions = computed(() => [
  { label: '无环境', value: '' },
  ...workspace.environments.map((item) => ({ label: item.name, value: item.id })),
])

function selectEnvironment(id: string | null) {
  workspace.selectEnvironment(id || null)
}

function updateEnvironmentName(value: string) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  workspace.updateEnvironment(environment.id, { name: value })
}

function updateVariable(index: number, field: 'key' | 'value', value: string) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  const delta: Partial<EnvironmentVariable> = { [field]: value }
  workspace.updateEnvironmentVariable(environment.id, index, delta)
}

function updateEnabled(index: number, enabled: boolean) {
  const environment = workspace.activeEnvironment
  if (!environment) return
  workspace.updateEnvironmentVariable(environment.id, index, { enabled })
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
      <AppSelect
        class="environment-select-control"
        :value="workspace.activeEnvironmentId ?? ''"
        :options="environmentOptions"
        aria-label="请求变量环境"
        @update:value="selectEnvironment"
      />
    </label>
    <AppPopover
      v-model:show="open"
      trigger="click"
      placement="bottom-end"
      :show-arrow="false"
      :style="{ width: 'min(430px, calc(100vw - 24px))' }"
    >
      <template #trigger>
        <AppButton
          class="variables-button"
          size="small"
          :disabled="!workspace.available"
          :aria-expanded="open"
          title="编辑环境变量"
        >
          变量
        </AppButton>
      </template>
      <div class="environment-popover" role="dialog" aria-label="环境变量">
      <template v-if="workspace.activeEnvironment">
        <div class="environment-head">
          <AppInput
            class="environment-name"
            :value="workspace.activeEnvironment.name"
            aria-label="环境名称"
            @update:value="updateEnvironmentName"
          />
          <AppButton text type="primary" size="small" @click="workspace.saveEnvironment()">保存</AppButton>
        </div>
        <div class="environment-rows">
          <div
            v-for="(variable, index) in workspace.activeEnvironment.variables"
            :key="`${workspace.activeEnvironment.id}-${index}`"
            class="environment-row"
          >
            <AppCheckbox
              :checked="variable.enabled"
              :aria-label="`启用变量 ${variable.key || '空变量'}`"
              @update:checked="updateEnabled(index, $event)"
            />
            <AppInput
              class="environment-key"
              :value="variable.key"
              placeholder="变量名"
              :input-props="{ spellcheck: false }"
              @update:value="updateVariable(index, 'key', $event)"
            />
            <AppInput
              class="environment-value"
              :value="variable.value"
              placeholder="变量值"
              :input-props="{ spellcheck: false }"
              @update:value="updateVariable(index, 'value', $event)"
            />
            <AppButton
              class="delete"
              quaternary
              circle
              size="small"
              aria-label="删除变量"
              @click="workspace.removeEnvironmentVariable(workspace.activeEnvironment.id, index)"
            >
              <template #icon><AppIcon :component="IconDelete" :size="15" /></template>
            </AppButton>
          </div>
        </div>
        <AppButton
          text
          type="primary"
          size="small"
          @click="workspace.addEnvironmentVariable(workspace.activeEnvironment.id)"
        >
          <template #icon><AppIcon :component="IconAdd" :size="14" /></template>
          变量
        </AppButton>
      </template>
      <template v-else>
        <p class="environment-empty">还没有环境。</p>
        <AppButton text type="primary" size="small" @click="createEnvironment">
          <template #icon><AppIcon :component="IconAdd" :size="14" /></template>
          创建环境
        </AppButton>
      </template>
      </div>
    </AppPopover>
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
.environment-select-control { width: 128px; }
.environment-select-control :deep(.n-base-selection-label) { font-family: var(--a-font-mono); }
.variables-button { font-family: var(--a-font-mono); }
.environment-popover {
  width: 100%;
}
.environment-head { display: flex; align-items: center; gap: 8px; padding-bottom: 7px; border-bottom: 1px solid var(--a-border); }
.environment-name { flex: 1; min-width: 0; }
.environment-rows { display: flex; flex-direction: column; gap: 5px; max-height: 260px; padding: 8px 0; overflow: auto; }
.environment-row { display: flex; align-items: center; gap: 4px; min-width: 0; }
.environment-key,
.environment-value { min-width: 0; }
.environment-key :deep(.n-input__input-el),
.environment-value :deep(.n-input__input-el),
.environment-name :deep(.n-input__input-el) { font-family: var(--a-font-mono); }
.environment-key { flex: 0 0 118px; }
.environment-value { flex: 1 1 auto; }
.delete { flex: 0 0 auto; color: var(--a-text-faint); }
.delete:hover { color: var(--a-error); }
.environment-empty { margin: 4px 0 8px; color: var(--a-text-dim); font-size: 12px; }
</style>
