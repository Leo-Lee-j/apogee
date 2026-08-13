<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { AppAlert, AppButton, AppIcon } from '@/components/ui'
import { IconArrowLeft, IconCloudUpload } from '@/components/ui/icons'
import { ipc, type YapiAnalyzeResult, type YapiImportResult } from '@/ipc'
import { useWorkspaceStore } from '@/stores/workspace'

const emit = defineEmits<{
  back: []
  imported: [result: YapiImportResult]
}>()

const workspace = useWorkspaceStore()
const baseUrl = ref('')
const token = ref('')
const collectionName = ref('')
const importEnvironments = ref(true)
const analysis = ref<YapiAnalyzeResult | null>(null)
const busy = ref(false)
const errorMessage = ref('')

const canAnalyze = computed(() => baseUrl.value.trim() !== '' && token.value.trim() !== '')
const canApply = computed(() => Boolean(analysis.value) && collectionName.value.trim() !== '')

function errorText(error: unknown, fallback: string) {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  if (error && typeof error === 'object') {
    const message = Reflect.get(error, 'message')
    if (typeof message === 'string') return message
  }
  return fallback
}

async function discardAnalysis() {
  const planId = analysis.value?.planId
  analysis.value = null
  if (!planId) return
  try {
    await ipc.yapiDiscardImport(planId)
  } catch {
    // 临时计划也会随进程结束释放；清理失败不应阻止用户离开向导。
  }
}

async function goBack() {
  await discardAnalysis()
  emit('back')
}

onBeforeUnmount(() => {
  const planId = analysis.value?.planId
  if (planId) void ipc.yapiDiscardImport(planId).catch(() => undefined)
})

async function analyzeProject() {
  if (!canAnalyze.value || busy.value) return
  busy.value = true
  errorMessage.value = ''
  analysis.value = null
  try {
    const result = await ipc.yapiAnalyzeImport(baseUrl.value.trim(), token.value)
    token.value = ''
    analysis.value = result
    collectionName.value = result.projectName
  } catch (error) {
    errorMessage.value = errorText(error, '无法分析 YApi 项目，请检查地址、Token 和网络。')
  } finally {
    busy.value = false
  }
}

async function applyImport() {
  if (!analysis.value || !canApply.value || busy.value) return
  busy.value = true
  errorMessage.value = ''
  try {
    const result = await ipc.yapiApplyImport(analysis.value.planId, {
      collectionName: collectionName.value.trim(),
      importEnvironments: importEnvironments.value,
    })
    await workspace.load()
    analysis.value = null
    emit('imported', result)
  } catch (error) {
    errorMessage.value = errorText(error, 'YApi 项目导入失败，工作区未发生变化。')
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="yapi-panel">
    <div class="yapi-content">
      <section class="form-section" aria-labelledby="yapi-source-title">
        <h3 id="yapi-source-title">连接 YApi 项目</h3>
        <p class="help">Token 只用于本次分析，不会写入工作区、配置或导入报告。</p>
        <div class="fields">
          <label>
            <span>实例地址或项目页面地址</span>
            <input
              v-model="baseUrl"
              type="url"
              autocomplete="url"
              spellcheck="false"
              placeholder="https://yapi.example.com"
              :disabled="busy || Boolean(analysis)"
            >
          </label>
          <label>
            <span>项目 Token</span>
            <input
              v-model="token"
              type="password"
              autocomplete="off"
              spellcheck="false"
              placeholder="输入项目 Token"
              :disabled="busy || Boolean(analysis)"
              @keyup.enter="analyzeProject"
            >
          </label>
        </div>
      </section>

      <AppAlert v-if="errorMessage" type="error" :show-icon="false" closable @close="errorMessage = ''">
        {{ errorMessage }}
      </AppAlert>

      <section v-if="analysis" class="preview" aria-labelledby="yapi-preview-title">
        <div class="preview-head">
          <div>
            <h3 id="yapi-preview-title">导入预览</h3>
            <p>{{ analysis.projectName }}</p>
          </div>
          <button class="reanalyze" type="button" :disabled="busy" @click="discardAnalysis">重新分析</button>
        </div>
        <div class="metrics">
          <div><strong>{{ analysis.categories.length }}</strong><span>分类</span></div>
          <div><strong>{{ analysis.requestCount }}</strong><span>接口</span></div>
          <div><strong>{{ analysis.environmentCount }}</strong><span>环境</span></div>
        </div>
        <div class="category-list">
          <div v-for="category in analysis.categories" :key="category.name">
            <span>{{ category.name }}</span><span>{{ category.requestCount }} 个接口</span>
          </div>
          <p v-if="analysis.categories.length === 0">没有可导入的接口</p>
        </div>
        <AppAlert v-if="analysis.warnings.length" type="warning" :show-icon="false">
          <strong>转换提示</strong>
          <ul><li v-for="warning in analysis.warnings" :key="warning">{{ warning }}</li></ul>
        </AppAlert>
        <div class="options">
          <label>
            <span>新集合名称</span>
            <input v-model="collectionName" maxlength="120" :disabled="busy">
          </label>
          <label class="check-row">
            <input v-model="importEnvironments" type="checkbox" :disabled="busy || analysis.environmentCount === 0">
            <span>导入 {{ analysis.environmentCount }} 个环境及变量</span>
          </label>
        </div>
      </section>
    </div>

    <div class="actions">
      <AppButton secondary :disabled="busy" @click="goBack">
        <template #icon><AppIcon :component="IconArrowLeft" :size="15" /></template>
        返回
      </AppButton>
      <AppButton
        v-if="!analysis"
        type="primary"
        :loading="busy"
        :disabled="!canAnalyze"
        @click="analyzeProject"
      >
        分析项目
      </AppButton>
      <AppButton
        v-else
        type="primary"
        :loading="busy"
        :disabled="!canApply || analysis.requestCount === 0"
        @click="applyImport"
      >
        <template #icon><AppIcon :component="IconCloudUpload" :size="15" /></template>
        确认导入
      </AppButton>
    </div>
  </div>
</template>

<style scoped>
.yapi-panel { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; padding: 4px 16px 16px; gap: 10px; }
.yapi-content { flex: 1 1 auto; min-height: 0; overflow: auto; display: grid; align-content: start; gap: 12px; }
.form-section, .preview { padding: 12px; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); background: var(--a-bg-3); }
h3 { margin: 0; color: var(--a-text); font: var(--a-fw-semibold) var(--a-fs-base)/1.4 var(--a-font-ui); }
.help, .preview-head p { margin: 3px 0 0; color: var(--a-text-dim); font: var(--a-fs-sm)/1.5 var(--a-font-ui); }
.fields, .options { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; margin-top: 12px; }
label { display: grid; gap: 5px; color: var(--a-text-dim); font: var(--a-fw-medium) var(--a-fs-sm)/var(--a-lh-sm) var(--a-font-ui); }
input[type='url'], input[type='password'], input:not([type]) { width: 100%; min-height: var(--a-h-input-md); box-sizing: border-box; padding: 0 9px; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); background: var(--a-bg-2); color: var(--a-text); font: var(--a-fs-sm)/var(--a-lh-sm) var(--a-font-ui); }
input:focus { outline: 2px solid var(--a-focus); outline-offset: -1px; }
input:disabled { opacity: .65; cursor: not-allowed; }
.preview-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 12px; }
.reanalyze { padding: 0; border: 0; background: transparent; color: var(--a-primary); cursor: pointer; font: var(--a-fs-sm)/1.4 var(--a-font-ui); }
.metrics { display: grid; grid-template-columns: repeat(3, 1fr); gap: 8px; margin: 12px 0; }
.metrics div { display: flex; align-items: baseline; gap: 6px; padding: 9px; border-radius: var(--a-radius-sm); background: var(--a-bg-2); }
.metrics strong { color: var(--a-text); font: var(--a-fw-semibold) var(--a-fs-lg)/1 var(--a-font-mono); }
.metrics span { color: var(--a-text-dim); font-size: var(--a-fs-xs); }
.category-list { max-height: 180px; overflow: auto; border: 1px solid var(--a-border); border-radius: var(--a-radius-sm); }
.category-list div { display: flex; justify-content: space-between; gap: 12px; padding: 7px 9px; color: var(--a-text-soft); font: var(--a-fs-sm)/1.4 var(--a-font-ui); }
.category-list div + div { border-top: 1px solid var(--a-border); }
.category-list div span:last-child { flex: 0 0 auto; color: var(--a-text-faint); }
.category-list p { margin: 0; padding: 10px; color: var(--a-text-faint); font-size: var(--a-fs-sm); }
ul { margin: 5px 0 0; padding-left: 18px; }
.check-row { grid-template-columns: auto 1fr; align-items: center; align-self: end; min-height: calc(var(--a-h-control-sm) + 2px); }
.check-row input { margin: 0; accent-color: var(--a-primary); }
.actions { flex: 0 0 auto; display: flex; justify-content: space-between; gap: 8px; }
@media (max-width: 720px) { .fields, .options { grid-template-columns: minmax(0, 1fr); } }
</style>
