<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { AppAlert, AppButton, AppInput, AppModal, AppSpace } from '@/components/ui'
import { AppIcon } from '@/components/ui'
import { IconAdd, IconFolder, IconFolderOpen } from '@/components/ui/icons'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
// feedback 带 naive-ui message 运行时，保持动态 import，避免拖进首屏 chunk（S5 变体 b）。

/**
 * 「保存到」位置选择器：草稿标签（没有 nodeId）点保存时弹出来，
 * 让用户选一个集合/文件夹，把当前编辑内容存成一个集合内节点。
 *
 * 只把集合和文件夹作为目标（`isFolder` 为真的扁平节点），请求节点不参与选择。
 * 空态引导建集合；名称留空时禁用保存（Rust 侧 `required_name` 也会拒绝空名）。
 */
const props = defineProps<{ show: boolean }>()
const emit = defineEmits<{ 'update:show': [value: boolean] }>()

const tabs = useTabsStore()
const workspace = useWorkspaceStore()

const name = ref('')
const selectedId = ref<string | null>(null)
const busy = ref(false)
const error = ref('')

/** 可保存的目标：集合 + 文件夹（扁平节点已按树序排列，直接用 depth 缩进渲染）。 */
const targets = computed(() => (workspace.nodes ?? []).filter((node) => node.isFolder))
const firstCollectionId = computed(
  () => (workspace.nodes ?? []).find((node) => node.kind === 'collection')?.id ?? null,
)
const canConfirm = computed(
  () => !busy.value && selectedId.value !== null && name.value.trim().length > 0,
)

/** 每次打开时重置状态，并把默认选中项指向创建来源或第一个集合。 */
function reset() {
  const tab = tabs.active
  name.value = tab?.title || '新建请求'
  selectedId.value = tab?.newRequestParentId ?? firstCollectionId.value
  error.value = ''
  busy.value = false
}

watch(
  () => props.show,
  (open) => {
    if (open) reset()
  },
)

function close() {
  if (busy.value) return
  emit('update:show', false)
}

async function onConfirm() {
  const tab = tabs.active
  const target = selectedId.value
  if (!tab || !target || !canConfirm.value) return
  busy.value = true
  error.value = ''
  try {
    const ok = await workspace.saveTabAs(tab, target, name.value.trim())
    if (ok) {
      emit('update:show', false)
      const { toast } = await import('@/features/feedback')
      toast().success('已保存')
    } else {
      error.value = workspace.lastError?.message ?? '保存失败，请重试。'
    }
  } finally {
    busy.value = false
  }
}

async function createCollection() {
  if (busy.value) return
  busy.value = true
  error.value = ''
  try {
    if (await workspace.createCollection()) {
      selectedId.value = firstCollectionId.value
    } else {
      error.value = workspace.lastError?.message ?? '创建集合失败。'
    }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <NaiveThemeScope>
    <AppModal
      :show="props.show"
      preset="card"
      title="保存到"
      :style="{ width: 'min(440px, calc(100vw - 32px))' }"
      :mask-closable="!busy"
      :closable="!busy"
      @update:show="emit('update:show', $event)"
    >
      <div class="save-picker">
        <label class="field-label" for="save-name">请求名称</label>
        <AppInput
          id="save-name"
          v-model:value="name"
          autofocus
          clearable
          maxlength="120"
          :input-props="{ spellcheck: false }"
          placeholder="请求名称"
        />

        <label class="field-label" id="save-target-label">保存位置</label>
        <AppAlert v-if="error" class="error" type="error" :show-icon="false" closable @close="error = ''">
          {{ error }}
        </AppAlert>

        <div
          v-if="targets.length"
          class="target-list"
          role="listbox"
          aria-labelledby="save-target-label"
          aria-label="选择保存位置"
        >
          <button
            v-for="node in targets"
            :key="node.id"
            type="button"
            class="target-row"
            :class="{ selected: node.id === selectedId }"
            :aria-selected="node.id === selectedId"
            role="option"
            :style="{ paddingLeft: `${8 + node.depth * 14}px` }"
            @click="selectedId = node.id"
          >
            <span class="t-icon">
              <AppIcon :component="node.kind === 'collection' ? IconFolder : IconFolderOpen" :size="15" />
            </span>
            <span class="t-name">{{ node.name }}</span>
          </button>
        </div>

        <div v-else class="empty">
          <p v-if="!workspace.available">
            保存到集合仅在桌面应用中可用（当前运行于浏览器预览）。
          </p>
          <template v-else>
            <p>还没有集合。先创建一个集合，再把请求存进去。</p>
            <AppButton secondary size="small" :loading="busy" @click="createCollection">
              <template #icon><AppIcon :component="IconAdd" :size="14" /></template>
              新建集合
            </AppButton>
          </template>
        </div>
      </div>

      <template #footer>
        <AppSpace justify="end">
          <AppButton :disabled="busy" @click="close">取消</AppButton>
          <AppButton type="primary" :loading="busy" :disabled="!canConfirm" @click="onConfirm">
            保存
          </AppButton>
        </AppSpace>
      </template>
    </AppModal>
  </NaiveThemeScope>
</template>

<style scoped>
.save-picker { display: flex; flex-direction: column; gap: 8px; }
.field-label {
  color: var(--a-text-dim);
  font: var(--a-fw-medium) var(--a-fs-sm) / 1 var(--a-font-ui);
}
/* 名称输入框沿用应用统一的字段输入样式（与 Sidebar / EnvironmentControls 一致）：
   不单独设置时 naive-ui 默认 36px/10px padding，与其它字段并排时明显矮一截。 */
.save-picker :deep(.n-input) { min-height: 38px; }
.save-picker :deep(.n-input-wrapper) { padding-inline: 12px; }
.error { flex: 0 0 auto; }
.target-list {
  max-height: 260px;
  overflow-y: auto;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-2);
  padding: 4px;
}
.target-row {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  box-sizing: border-box;
  height: 28px;
  padding: 0 8px;
  border: 0;
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text);
  font: var(--a-fs-sm) / 1 var(--a-font-ui);
  text-align: left;
  cursor: pointer;
}
.target-row:hover { background: var(--a-hover); }
.target-row.selected { background: var(--a-selected); }
.t-icon { flex: 0 0 auto; display: inline-flex; color: var(--a-primary); }
.t-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.empty {
  padding: 10px 12px;
  border: 1px dashed var(--a-border-strong);
  border-radius: var(--a-radius-md);
  color: var(--a-text-dim);
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 8px;
}
.empty p { margin: 0; }
</style>
