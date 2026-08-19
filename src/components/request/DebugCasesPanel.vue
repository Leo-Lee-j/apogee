<script setup lang="ts">
import { computed, h, ref, watch, type Component } from 'vue'
import type { SelectOption } from 'naive-ui'
import {
  AppButton,
  AppIcon,
  AppInput,
  AppModal,
  AppSelect,
  AppSpace,
} from '@/components/ui'
import { IconCloseCircle, IconFlash, IconShield } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { type DebugCaseType, type RequestSpec } from '@/ipc'
import { toast } from '@/features/feedback'
import { useWorkspaceStore } from '@/stores/workspace'

const props = defineProps<{
  show: boolean
  /** 请求 YAML 的稳定身份；草稿请求应由入口先提示并阻止打开。 */
  requestId?: string
  requestName: string
  spec: RequestSpec
}>()

const emit = defineEmits<{
  'update:show': [show: boolean]
}>()

const workspace = useWorkspaceStore()
const visible = computed({
  get: () => props.show,
  set: (show: boolean) => emit('update:show', show),
})
const name = ref('')
const caseType = ref<DebugCaseType>('success')
const creating = ref(false)

type CaseTypeOption = {
  value: DebugCaseType
  label: string
  description: string
  color: string
  icon: Component
}

const CASE_TYPE_OPTIONS: CaseTypeOption[] = [
  {
    value: 'success',
    label: '成功用例',
    description: '覆盖预期正常完成的场景',
    color: 'var(--a-ok)',
    icon: IconShield,
  },
  {
    value: 'failure',
    label: '失败用例',
    description: '覆盖预期业务失败的场景',
    color: 'var(--a-warn)',
    icon: IconCloseCircle,
  },
  {
    value: 'exception',
    label: '异常用例',
    description: '覆盖超时、网络等异常场景',
    color: 'var(--a-error)',
    icon: IconFlash,
  },
]

const selectedType = computed(
  () => CASE_TYPE_OPTIONS.find((option) => option.value === caseType.value) ?? CASE_TYPE_OPTIONS[0]!,
)

function setCaseType(value: unknown) {
  if (value === 'success' || value === 'failure' || value === 'exception') caseType.value = value
}

function renderCaseTypeLabel(option: SelectOption, selected: boolean) {
  const type = typeof option.value === 'string'
    ? CASE_TYPE_OPTIONS.find((item) => item.value === option.value)
    : undefined
  if (!type) return option.label as string

  return h('span', {
    style: {
      display: 'inline-flex',
      alignItems: 'center',
      gap: '6px',
      width: '100%',
      color: type.color,
      fontWeight: selected ? 'var(--a-fw-semibold)' : 'var(--a-fw-medium)',
    },
  }, [
    h(AppIcon, { component: type.icon, size: 16, 'aria-hidden': true }),
    h('span', type.label),
  ])
}

function suggestedName() {
  return props.requestName && props.requestName !== '新建请求'
    ? `${props.requestName} 用例`
    : '调试用例'
}

async function createCase() {
  const requestId = props.requestId
  const caseName = name.value.trim()
  if (creating.value || !caseName) return
  if (!requestId) {
    toast().warning('请先保存请求，再保存调试用例。')
    visible.value = false
    return
  }

  creating.value = true
  try {
    const debugCase = await workspace.createDebugCase(
      requestId,
      caseName,
      caseType.value,
      structuredClone(props.spec),
    )
    if (!debugCase) {
      toast().error(workspace.lastError?.message ?? '保存用例失败，请稍后重试。')
      return
    }
    toast().success(`已保存用例：“${debugCase.name}”`)
    visible.value = false
  } finally {
    creating.value = false
  }
}

watch(
  () => [visible.value, props.requestId] as const,
  ([show]) => {
    if (!show) return
    name.value = suggestedName()
    caseType.value = 'success'
  },
  { immediate: true },
)
</script>

<template>
  <NaiveThemeScope>
    <AppModal
      v-model:show="visible"
      preset="card"
      :title="`保存用例 · ${props.requestName}`"
      :style="{ width: 'min(520px, calc(100vw - 32px))' }"
      :mask-closable="!creating"
      :closable="!creating"
    >
      <div class="save-case-form">
        <label class="form-field">
          <span>用例名称</span>
          <AppInput
            v-model:value="name"
            autofocus
            clearable
            :disabled="creating"
            placeholder="例如：不存在的用户"
            aria-label="调试用例名称"
            @keyup.enter="createCase"
          />
        </label>

        <label class="form-field">
          <span>用例类型</span>
          <AppSelect
            class="case-type-select"
            :value="caseType"
            :options="CASE_TYPE_OPTIONS"
            :render-label="renderCaseTypeLabel"
            :disabled="creating"
            aria-label="用例类型"
            @update:value="setCaseType"
          />
        </label>

        <p class="type-hint" :style="{ color: selectedType.color }">
          <AppIcon :component="selectedType.icon" :size="15" aria-hidden="true" />
          {{ selectedType.description }}；类型只用于分类，不作为运行断言。
        </p>
      </div>

      <template #footer>
        <AppSpace justify="end">
          <AppButton :disabled="creating" @click="visible = false">取消</AppButton>
          <AppButton type="primary" :loading="creating" :disabled="!name.trim()" @click="createCase">保存用例</AppButton>
        </AppSpace>
      </template>
    </AppModal>
  </NaiveThemeScope>
</template>

<style scoped>
.save-case-form { display: grid; gap: 14px; }
.form-field { display: grid; gap: 7px; color: var(--a-text-soft); font: var(--a-fs-sm)/var(--a-lh-sm) var(--a-font-ui); }
.case-type-select { width: 100%; }
.type-hint { display: flex; align-items: center; gap: 6px; margin: 0; font: var(--a-fs-xs)/var(--a-lh-sm) var(--a-font-ui); }
</style>
