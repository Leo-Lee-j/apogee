<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import {
  AppAlert,
  AppButton,
  AppForm,
  AppFormItem,
  AppInputNumber,
  AppInput,
  AppPopconfirm,
  AppSelect,
  AppSpace,
  AppSwitch,
} from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { AppIcon } from '@/components/ui'
import {
  IconFlash,
  IconFolderOpen,
  IconInfo,
  IconNetwork,
  IconPalette,
  IconReset,
  IconRestart,
} from '@/components/ui/icons'
import { ipc } from '@/ipc'
import { useUiStore } from '@/stores/ui'
import { useAppStore } from '@/stores/app'

/** 设置左侧导航分区；命令面板 / openSettings 可直达。 */
export type SettingsSection = 'appearance' | 'startup' | 'network' | 'performance' | 'about'

const LABEL_WIDTH = 140
const PROXY_SAVE_DEBOUNCE_MS = 400
const APP_VERSION = '0.1.0'

const ui = useUiStore()
const app = useAppStore()

const activeSection = ref<SettingsSection>(ui.settingsSection)
const proxyUrlDraft = ref(app.settings.proxyUrl ?? '')
const settingsError = ref<string | null>(null)
let proxySaveTimer: ReturnType<typeof setTimeout> | null = null

const sections = [
  { id: 'appearance' as const, label: '外观', icon: IconPalette },
  { id: 'startup' as const, label: '启动与工作区', icon: IconFolderOpen },
  { id: 'network' as const, label: '网络', icon: IconNetwork },
  { id: 'performance' as const, label: '性能', icon: IconFlash },
  { id: 'about' as const, label: '关于', icon: IconInfo },
]

const themeOptions = [
  { label: '暗色', value: 'dark' },
  { label: '亮色', value: 'light' },
  { label: '跟随系统', value: 'system' },
]
const proxyOptions = [
  { label: '系统代理', value: 'system' },
  { label: '手动配置', value: 'manual' },
  { label: '不使用代理', value: 'none' },
]

const proxyModeLabel = computed(() => {
  switch (app.settings.proxyMode) {
    case 'manual':
      return '手动'
    case 'none':
      return '关闭'
    default:
      return '系统'
  }
})

watch(
  () => ui.settingsSection,
  (section) => {
    if (section !== activeSection.value) activeSection.value = section
  },
)

watch(activeSection, (section) => {
  if (ui.settingsSection !== section) ui.settingsSection = section
})

watch(
  () => app.settings.proxyUrl,
  (value) => {
    const next = value ?? ''
    if (next !== proxyUrlDraft.value) proxyUrlDraft.value = next
  },
)

onBeforeUnmount(() => {
  if (proxySaveTimer) {
    clearTimeout(proxySaveTimer)
    proxySaveTimer = null
  }
})

function selectSection(section: SettingsSection) {
  activeSection.value = section
  ui.settingsSection = section
  settingsError.value = null
}

async function persist(patch: Partial<typeof app.settings>) {
  const ok = await app.saveSettings({ ...app.settings, ...patch })
  if (!ok) {
    settingsError.value = app.lastError?.message ?? '保存设置失败'
    return false
  }
  settingsError.value = null
  return true
}

async function chooseWorkspaceRoot() {
  const { open } = await import('@tauri-apps/plugin-dialog')
  const selected = await open({ directory: true, multiple: false })
  if (typeof selected === 'string') {
    await persist({ defaultWorkspaceRoot: selected })
  }
}

async function setAutoOpen(value: boolean) {
  await persist({ openLastWorkspaceOnStart: value })
}

async function setMemoryMonitorVisible(value: boolean) {
  await persist({ showMemoryMonitor: value })
}

async function setLowMemoryMode(value: boolean) {
  await persist({ lowMemoryMode: value })
}

async function setNetwork(patch: Partial<typeof app.settings>) {
  await persist(patch)
}

function onProxyUrlInput(value: string) {
  proxyUrlDraft.value = value
  if (proxySaveTimer) clearTimeout(proxySaveTimer)
  proxySaveTimer = setTimeout(() => {
    proxySaveTimer = null
    void persist({ proxyUrl: value })
  }, PROXY_SAVE_DEBOUNCE_MS)
}

async function flushProxyUrl() {
  if (proxySaveTimer) {
    clearTimeout(proxySaveTimer)
    proxySaveTimer = null
  }
  const value = proxyUrlDraft.value
  if ((app.settings.proxyUrl ?? '') !== value) {
    await persist({ proxyUrl: value })
  }
}

async function restartApp() {
  await ipc.appRestart()
}
</script>

<template>
  <NaiveThemeScope>
    <section class="settings-page a-block" aria-label="设置">
      <aside class="settings-nav" aria-label="设置分类">
        <div class="nav-heading">
          <div class="nav-title">设置</div>
          <div class="nav-subtitle">偏好与应用配置</div>
        </div>
        <button
          v-for="section in sections"
          :key="section.id"
          type="button"
          :class="{ active: activeSection === section.id }"
          @click="selectSection(section.id)"
        >
          <AppIcon :component="section.icon" :size="16" />
          <span>{{ section.label }}</span>
        </button>
      </aside>

      <div class="settings-content">
        <div class="settings-scroll">
          <AppAlert v-if="settingsError" class="settings-banner" type="error" :bordered="false">
            {{ settingsError }}
          </AppAlert>

          <template v-if="activeSection === 'appearance'">
            <header class="section-head">
              <h1>外观</h1>
              <p>控制应用主题。请求/响应分栏可在状态栏切换。</p>
            </header>

            <section class="settings-group">
              <div class="group-heading">
                <h2>界面</h2>
                <p>主题立即生效，并作用于整个应用窗口。</p>
              </div>
              <AppForm label-placement="left" :label-width="LABEL_WIDTH" size="medium">
                <AppFormItem label="主题">
                  <AppSelect
                    class="control"
                    size="medium"
                    :value="ui.themeMode"
                    :options="themeOptions"
                    @update:value="(value: any) => { ui.themeMode = value; ui.applyTheme() }"
                  />
                </AppFormItem>
              </AppForm>
            </section>

            <section class="settings-group">
              <div class="group-heading">
                <h2>侧边栏</h2>
                <p>恢复工作区导航栏的默认宽度。</p>
              </div>
              <AppButton secondary @click="ui.resetSidebarWidth()">
                <template #icon><AppIcon :component="IconReset" :size="16" /></template>
                恢复默认宽度
              </AppButton>
            </section>
          </template>

          <template v-else-if="activeSection === 'startup'">
            <header class="section-head">
              <h1>启动与工作区</h1>
              <p>控制启动后首先显示的界面，以及新建工作区的默认位置。</p>
            </header>

            <section class="settings-group">
              <div class="group-heading">
                <h2>启动</h2>
                <p>关闭后每次启动都会回到工作区选择页。</p>
              </div>
              <AppForm label-placement="left" :label-width="LABEL_WIDTH" size="medium">
                <AppFormItem label="打开上次工作区">
                  <div class="setting-inline">
                    <AppSwitch
                      :value="app.settings.openLastWorkspaceOnStart"
                      @update:value="setAutoOpen"
                    />
                    <span>启动时自动进入最近一次可用的工作区</span>
                  </div>
                </AppFormItem>
              </AppForm>
            </section>

            <section class="settings-group">
              <div class="group-heading">
                <h2>默认工作区位置</h2>
                <p>新建工作区默认保存到此目录，已有工作区不会被移动。</p>
              </div>
              <AppForm label-placement="left" :label-width="LABEL_WIDTH" size="medium">
                <AppFormItem label="保存位置" :show-feedback="false">
                  <AppSpace :wrap="false" :size="8" align="center" class="control-row">
                    <AppInput
                      class="control-grow"
                      size="medium"
                      :value="app.settings.defaultWorkspaceRoot"
                      readonly
                      :input-props="{ spellcheck: false }"
                    />
                    <AppButton secondary size="medium" @click="chooseWorkspaceRoot">更改位置</AppButton>
                  </AppSpace>
                </AppFormItem>
              </AppForm>
            </section>
          </template>

          <template v-else-if="activeSection === 'network'">
            <header class="section-head">
              <h1>网络</h1>
              <p>设置请求使用的代理、超时和证书校验策略。</p>
            </header>

            <section class="settings-group">
              <div class="group-heading">
                <h2>连接</h2>
                <p>当前代理：{{ proxyModeLabel }}。状态栏会同步显示。</p>
              </div>
              <AppForm label-placement="left" :label-width="LABEL_WIDTH" size="medium">
                <AppFormItem label="代理">
                  <AppSelect
                    class="control"
                    size="medium"
                    :value="app.settings.proxyMode"
                    :options="proxyOptions"
                    @update:value="(value: any) => setNetwork({ proxyMode: value })"
                  />
                </AppFormItem>
                <AppFormItem v-if="app.settings.proxyMode === 'manual'" label="代理地址" :show-feedback="false">
                  <AppInput
                    class="control-wide"
                    size="medium"
                    :value="proxyUrlDraft"
                    placeholder="http://127.0.0.1:8080"
                    :input-props="{ spellcheck: false }"
                    @update:value="onProxyUrlInput"
                    @blur="flushProxyUrl"
                  />
                </AppFormItem>
                <AppFormItem label="超时">
                  <div class="setting-inline">
                    <AppInputNumber
                      :value="app.settings.timeoutMs"
                      class="control-num"
                      size="medium"
                      :min="0"
                      :step="1000"
                      @update:value="(value: number | null) => setNetwork({ timeoutMs: value ?? 30000 })"
                    />
                    <span class="unit">ms</span>
                    <span class="hint">整请求超时（含连接与下载）</span>
                  </div>
                </AppFormItem>
                <AppFormItem label="校验 TLS">
                  <AppSwitch
                    :value="app.settings.verifyTls"
                    @update:value="(value: boolean) => setNetwork({ verifyTls: value })"
                  />
                </AppFormItem>
              </AppForm>
              <AppAlert v-if="!app.settings.verifyTls" type="warning" :bordered="false">
                关闭 TLS 校验后，连接可能被中间人窃听。仅用于明确知道风险的测试环境。
              </AppAlert>
            </section>

            <section class="settings-group muted">
              <div class="group-heading">
                <h2>证书</h2>
                <p>自定义 CA 与客户端证书将在后续版本提供。</p>
              </div>
              <p class="placeholder">即将推出</p>
            </section>
          </template>

          <template v-else-if="activeSection === 'performance'">
            <header class="section-head">
              <h1>性能</h1>
              <p>内存相关选项不影响请求语义，只改变本机资源占用。</p>
            </header>

            <section class="settings-group">
              <div class="group-heading">
                <h2>内存</h2>
                <p>状态栏监测用于开发与自检；低内存模式适合长时间挂机。</p>
              </div>
              <AppForm label-placement="left" :label-width="LABEL_WIDTH" size="medium">
                <AppFormItem label="显示内存监测">
                  <AppSwitch
                    :value="app.settings.showMemoryMonitor"
                    @update:value="setMemoryMonitorVisible"
                  />
                </AppFormItem>
                <AppFormItem label="低内存模式">
                  <div class="setting-stack">
                    <div class="setting-inline">
                      <AppSwitch
                        :value="app.settings.lowMemoryMode"
                        @update:value="setLowMemoryMode"
                      />
                      <span>关闭 GPU 硬件加速，将在下次启动后生效</span>
                    </div>
                    <AppPopconfirm
                      positive-text="重启"
                      negative-text="取消"
                      @positive-click="restartApp"
                    >
                      <template #trigger>
                        <AppButton secondary size="small">
                          <template #icon><AppIcon :component="IconRestart" :size="14" /></template>
                          重启应用
                        </AppButton>
                      </template>
                      重启将应用已保存的设置，未保存的请求编辑可能丢失。
                    </AppPopconfirm>
                  </div>
                </AppFormItem>
              </AppForm>
            </section>
          </template>

          <template v-else>
            <header class="section-head">
              <h1>关于</h1>
              <p>版本与产品信息。</p>
            </header>

            <section class="settings-group">
              <div class="group-heading">
                <h2>应用</h2>
                <p>本地优先的桌面 API 客户端。</p>
              </div>
              <dl class="about-list">
                <div>
                  <dt>版本</dt>
                  <dd>v{{ APP_VERSION }}</dd>
                </div>
                <div>
                  <dt>产品定位</dt>
                  <dd class="plain">本地优先 · 启动即用</dd>
                </div>
              </dl>
            </section>
          </template>
        </div>
      </div>
    </section>
  </NaiveThemeScope>
</template>

<style scoped>
/* 外层走全局 .a-block（bg-2 / 圆角 / 阴影），这里只补设置页内部布局。 */
.settings-page {
  flex: 1 1 auto;
  display: grid;
  grid-template-columns: 208px minmax(0, 1fr);
  min-height: 0;
  overflow: hidden;
}
.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 16px 10px;
  border-right: 1px solid var(--a-border);
  background: transparent;
}
.nav-heading { padding: 0 10px 14px; }
.nav-title {
  color: var(--a-text);
  font-size: 16px;
  font-weight: var(--a-fw-semibold);
  line-height: 24px;
}
.nav-subtitle { margin-top: 2px; color: var(--a-text-dim); font-size: 12px; line-height: 18px; }
.settings-nav button {
  height: 36px;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 12px;
  border: 0;
  border-radius: var(--a-radius-md);
  background: transparent;
  color: var(--a-text-dim);
  font-size: 13px;
  font-weight: var(--a-fw-medium);
  text-align: left;
  cursor: pointer;
}
.settings-nav button:hover { background: var(--a-hover); color: var(--a-text); }
.settings-nav button.active {
  background: var(--a-hover);
  color: var(--a-text);
  box-shadow: inset 2px 0 0 var(--a-primary);
}
.settings-content {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: transparent;
}
.settings-scroll {
  flex: 1 1 auto;
  min-height: 0;
  max-width: 720px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  overflow: auto;
  padding: 24px 28px 32px;
}
.settings-banner { margin-bottom: 4px; }
.section-head { margin-bottom: 8px; }
.section-head h1 {
  margin: 0;
  color: var(--a-text);
  font-size: 18px;
  font-weight: var(--a-fw-semibold);
  line-height: 28px;
}
.section-head p {
  margin: 4px 0 0;
  color: var(--a-text-dim);
  font-size: 13px;
  line-height: 20px;
}
.settings-group {
  min-width: 0;
  padding: 16px 0;
  border-bottom: 1px solid var(--a-border);
}
.settings-group:last-child { border-bottom: 0; }
.settings-group.muted .placeholder {
  margin: 0;
  color: var(--a-text-faint);
  font-size: 13px;
  line-height: 20px;
}
.group-heading { margin-bottom: 14px; }
.group-heading h2 {
  margin: 0 0 4px;
  color: var(--a-text);
  font-size: 14px;
  line-height: 20px;
  font-weight: var(--a-fw-semibold);
}
.group-heading p {
  margin: 0;
  color: var(--a-text-dim);
  font-size: 13px;
  line-height: 20px;
}
.control { width: min(320px, 100%); }
.control-wide { width: min(480px, 100%); }
.control-num { width: 140px; }
.control-row {
  width: min(520px, 100%);
  min-width: 0;
}
.control-row :deep(.n-space) {
  width: 100%;
  flex-wrap: nowrap;
}
.control-grow {
  flex: 1 1 auto;
  min-width: 0;
  width: auto !important;
}
.control-grow :deep(.n-input) {
  width: 100%;
}
.unit {
  color: var(--a-text-faint);
  font: var(--a-fs-xs) var(--a-font-mono);
}
.hint { color: var(--a-text-faint); font-size: 12px; line-height: 18px; }
.setting-inline {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  color: var(--a-text-faint);
  font-size: 12px;
  line-height: 18px;
}
.setting-stack {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 10px;
}
.about-list {
  margin: 0;
  display: grid;
  gap: 10px;
  max-width: 420px;
}
.about-list > div {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  gap: 12px;
  align-items: baseline;
}
.about-list dt {
  margin: 0;
  color: var(--a-text-dim);
  font-size: 13px;
}
.about-list dd {
  margin: 0;
  color: var(--a-text);
  font-size: 13px;
  font-family: var(--a-font-mono);
}
.about-list dd.plain { font-family: var(--a-font-ui); }
.settings-group :deep(.n-form-item:last-child) { margin-bottom: 0; }
.settings-group :deep(.n-form-item-label) {
  color: var(--a-text-soft);
  font-weight: var(--a-fw-medium);
}

@media (max-width: 760px) {
  .settings-page {
    grid-template-columns: 1fr;
    grid-template-rows: auto minmax(0, 1fr);
  }
  .settings-nav {
    flex-direction: row;
    overflow-x: auto;
    padding: 10px;
    border-right: 0;
    border-bottom: 1px solid var(--a-border);
  }
  .nav-heading { display: none; }
  .settings-nav button { flex: 0 0 auto; }
  .settings-nav button.active { box-shadow: inset 0 -2px 0 var(--a-primary); }
  .settings-scroll { padding: 16px 16px 24px; }
  .control-row { width: 100%; }
  .control-row :deep(.n-space) { flex-wrap: wrap; }
  .control-grow { flex: 1 1 100%; }
  .about-list > div { grid-template-columns: 1fr; gap: 2px; }
}
</style>
