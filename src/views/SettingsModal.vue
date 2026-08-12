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
  SettingToggleCard,
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
            <section class="settings-group">
              <div class="group-heading">
                <h2>界面</h2>
                <p>主题立即生效，并作用于整个应用窗口。</p>
              </div>
              <div class="choice-grid" role="radiogroup" aria-label="主题">
                <button
                  v-for="option in themeOptions"
                  :key="option.value"
                  type="button"
                  class="choice-card"
                  :class="{ selected: ui.themeMode === option.value }"
                  role="radio"
                  :aria-checked="ui.themeMode === option.value"
                  @click="ui.themeMode = option.value as any; ui.applyTheme()"
                >
                  <span class="choice-title">{{ option.label }}</span>
                  <span class="choice-description">{{ option.value === 'dark' ? '适合低光环境' : option.value === 'light' ? '清晰明亮的界面' : '跟随系统设置' }}</span>
                </button>
              </div>
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
            <section class="settings-group">
              <div class="group-heading">
                <h2>启动</h2>
                <p>关闭后每次启动都会回到工作区选择页。</p>
              </div>
              <AppForm label-placement="left" :label-width="LABEL_WIDTH" size="medium">
                <SettingToggleCard title="打开上次工作区" description="启动时自动进入最近一次可用的工作区" :value="app.settings.openLastWorkspaceOnStart" @update:value="setAutoOpen" />
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
                <SettingToggleCard title="校验 TLS" description="验证 HTTPS 证书链，关闭后允许自签名证书。" :value="app.settings.verifyTls" @update:value="(value: boolean) => setNetwork({ verifyTls: value })" />
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
            <section class="settings-group">
              <div class="group-heading">
                <h2>内存</h2>
                <p>状态栏监测用于开发与自检；低内存模式适合长时间挂机。</p>
              </div>
              <AppForm label-placement="left" :label-width="LABEL_WIDTH" size="medium">
                <SettingToggleCard title="显示内存监测" description="在状态栏显示当前应用的内存使用情况。" :value="app.settings.showMemoryMonitor" @update:value="setMemoryMonitorVisible" />
                <SettingToggleCard title="低内存模式" description="关闭 GPU 硬件加速，将在下次启动后生效。" :value="app.settings.lowMemoryMode" @update:value="setLowMemoryMode">
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
                </SettingToggleCard>
              </AppForm>
            </section>
          </template>

          <template v-else>
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
  grid-template-columns: 180px minmax(0, 1fr);
  min-height: 0;
  overflow: hidden;
  background: var(--a-bg-2);
}
.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 20px 10px;
  border-right: 1px solid var(--a-border);
  background: var(--a-bg-3);
  width: 180px;
}
.nav-heading { padding: 0 10px 16px; }
.nav-title {
  color: var(--a-text);
  font-size: 16px;
  font-weight: var(--a-fw-semibold);
  line-height: 24px;
}
.nav-subtitle { margin-top: 2px; color: var(--a-text-faint); font-size: 12px; line-height: 18px; }
.settings-nav button {
  position: relative;
  height: 34px;
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 0 12px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--a-text-dim);
  font-size: 13px;
  font-weight: 500;
  text-align: left;
  cursor: pointer;
}
.settings-nav button:hover { background: var(--a-hover); color: var(--a-text); }
.settings-nav button.active {
  background: var(--a-selected);
  color: var(--a-primary);
  box-shadow: inset 0 0 0 1px var(--a-primary);
  font-weight: var(--a-fw-semibold);
}
.settings-content {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  background: var(--a-bg-2);
}
.settings-scroll {
  flex: 1 1 auto;
  min-height: 0;
  width: min(820px, 100%);
  display: flex;
  flex-direction: column;
  gap: 26px;
  overflow: auto;
  padding: 28px 32px 48px;
}
.settings-banner { margin-bottom: 4px; }
.settings-group {
  min-width: 0;
  padding: 0 0 22px;
  border: 0;
  border-bottom: 1px solid var(--a-border);
  background: transparent;
  border-radius: 0;
  box-shadow: none;
}
.settings-group.muted .placeholder {
  margin: 4px 0 0;
  color: var(--a-text-faint);
  font-size: 13px;
  line-height: 20px;
}
.settings-group.muted { opacity: .68; }
.group-heading { margin-bottom: 14px; }
.group-heading h2 {
  margin: 0 0 5px;
  color: var(--a-text);
  font-size: 14px;
  line-height: 20px;
  font-weight: 500;
}
.group-heading p {
  margin: 0;
  color: var(--a-text-dim);
  font-size: 13px;
  line-height: 20px;
}
.control { width: min(328px, 100%); }
.control-wide { width: min(472px, 100%); }
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
  gap: 0;
  max-width: 480px;
}
.about-list > div {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  gap: 12px;
  align-items: baseline;
  min-height: 36px;
  padding: 8px 0;
  border-top: 1px solid var(--a-border);
}
.about-list > div:first-child { border-top: 0; }
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
.settings-group :deep(.n-form-item) { margin-bottom: 14px; }
.settings-group :deep(.n-form-item-label) {
  color: var(--a-text-soft);
  font-weight: var(--a-fw-medium);
  font-size: 13px;
}
.choice-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; max-width: 640px; }
.choice-card { min-height: 68px; padding: 12px 14px; border: 1px solid var(--a-border-strong); border-radius: 6px; background: var(--a-bg-2); color: var(--a-text); text-align: left; cursor: pointer; }
.choice-card:hover { background: var(--a-hover); }
.choice-card.selected { background: var(--a-bg-3); border-color: var(--a-text-dim); }
.choice-title { display: block; font-size: 13px; font-weight: 500; line-height: 18px; }
.choice-description { display: block; margin-top: 4px; color: var(--a-text-dim); font-size: 12px; line-height: 16px; }
.settings-group :deep(.n-form-item) { margin-bottom: 12px; }
.settings-group :deep(.n-input), .settings-group :deep(.n-base-selection), .settings-group :deep(.n-button) { border-radius: 6px; }

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
  .settings-scroll { width: 100%; padding: 22px 16px 32px; }
  .control-row { width: 100%; }
  .control-row :deep(.n-space) { flex-wrap: wrap; }
  .control-grow { flex: 1 1 100%; }
  .about-list > div { grid-template-columns: 1fr; gap: 2px; }
}
</style>
