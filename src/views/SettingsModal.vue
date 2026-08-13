<script setup lang="ts">
import { computed, h, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  AppAlert,
  AppButton,
  AppForm,
  AppInputNumber,
  AppInput,
  AppPopconfirm,
  AppSelect,
  AppSwitch,
  SettingToggleCard,
} from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { AppIcon } from '@/components/ui'
import {
  IconFlash,
  IconFolderOpen,
  IconInfo,
  IconKeyboard,
  IconNetwork,
  IconPalette,
  IconReset,
  IconRestart,
} from '@/components/ui/icons'
import { ipc } from '@/ipc'
import { useUiStore } from '@/stores/ui'
import { useAppStore } from '@/stores/app'
import type { FontFamilyMode, LanguageMode, ShortcutId, ThemeMode } from '@/stores/ui'

/** 设置左侧导航分区；命令面板 / openSettings 可直达。 */
export type SettingsSection = 'appearance' | 'startup' | 'network' | 'performance' | 'shortcuts' | 'about'

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
  { id: 'shortcuts' as const, label: '快捷键', icon: IconKeyboard },
  { id: 'about' as const, label: '关于', icon: IconInfo },
]

const themeOptions = [
  { label: '暗色', value: 'dark' },
  { label: '亮色', value: 'light' },
  { label: '跟随系统', value: 'system' },
]
const languageOptions = [
  { label: '跟随系统', value: 'system' },
  { label: '简体中文', value: 'zh-CN' },
  { label: 'English', value: 'en-US' },
]
const fontOptions = ref([
  { label: '系统默认', value: 'system' },
])
const editorFontOptions = computed(() => [
  { label: '内置等宽字体（默认）', value: 'system' },
  ...fontOptions.value.filter((option) => option.value !== 'system'),
])
const proxyOptions = [
  { label: '系统代理', value: 'system' },
  { label: '手动配置', value: 'manual' },
  { label: '不使用代理', value: 'none' },
]

type ShortcutArea = '全局' | '工作区' | '请求编辑'
interface ShortcutItem {
  id: ShortcutId
  area: ShortcutArea
  name: string
  description: string
  keys: string[]
}

const shortcutSearch = ref('')
const shortcutSearchBy = ref<'name' | 'key'>('name')
const shortcuts: ShortcutItem[] = [
  { id: 'command-palette', area: '全局', name: '打开命令面板', description: '搜索并执行常用操作', keys: ['Ctrl', 'K'] },
  { id: 'settings', area: '全局', name: '打开设置', description: '进入应用偏好与配置', keys: ['Ctrl', ','] },
  { id: 'new-request', area: '工作区', name: '新建快捷请求', description: '创建空白请求并聚焦地址栏', keys: ['Ctrl', 'T'] },
  { id: 'toggle-sidebar', area: '工作区', name: '显示或隐藏侧边栏', description: '切换工作区导航栏', keys: ['Ctrl', 'B'] },
  { id: 'cycle-layout', area: '工作区', name: '切换请求与响应布局', description: '依次切换左右、上下和专注视图', keys: ['Ctrl', 'Shift', 'L'] },
  { id: 'save-request', area: '请求编辑', name: '保存请求', description: '保存当前请求或打开保存位置选择', keys: ['Ctrl', 'S'] },
  { id: 'send-request', area: '请求编辑', name: '发送请求', description: '发送当前正在编辑的请求', keys: ['Ctrl', 'Enter'] },
]
const shortcutAreas: ShortcutArea[] = ['全局', '工作区', '请求编辑']
const visibleShortcutGroups = computed(() => {
  const query = shortcutSearch.value.trim().toLowerCase()
  return shortcutAreas.map((area) => ({
    area,
    items: shortcuts.filter((shortcut) => {
      if (shortcut.area !== area) return false
      if (!query) return true
      const target = shortcutSearchBy.value === 'name'
        ? `${shortcut.name} ${shortcut.description}`.toLowerCase()
        : shortcut.keys.join('+').toLowerCase()
      return target.includes(query)
    }),
  })).filter((group) => group.items.length)
})

function shortcutKeyLabel(event: KeyboardEvent) {
  const key = event.key
  if (['Control', 'Shift', 'Alt', 'Meta'].includes(key)) return null
  const parts: string[] = []
  if (event.ctrlKey) parts.push('Ctrl')
  if (event.metaKey) parts.push('Meta')
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  const displayKey = key === ' ' ? 'Space' : key.length === 1 ? key.toUpperCase() : key
  parts.push(displayKey)
  return parts.join('+')
}

function captureShortcutSearch(event: KeyboardEvent) {
  if (shortcutSearchBy.value !== 'key') return
  if (event.key === 'Backspace' || event.key === 'Delete') {
    shortcutSearch.value = ''
    event.preventDefault()
    return
  }
  const key = shortcutKeyLabel(event)
  if (!key) return
  shortcutSearch.value = key
  event.preventDefault()
}

function setShortcutSearchMode(mode: 'name' | 'key') {
  shortcutSearchBy.value = mode
  shortcutSearch.value = ''
}

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

onMounted(async () => {
  try {
    const families = await ipc.systemFontFamilies()
    fontOptions.value = [
      { label: '系统默认', value: 'system' },
      ...families.map((family) => ({ label: family, value: family })),
    ]
  } catch {
    // IPC 不可用时保留系统默认和可输入模式，避免设置页不可用。
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

async function setTheme(value: ThemeMode) {
  ui.themeMode = value
  ui.applyTheme()
  await persist({ themeMode: value })
}

async function setLanguage(value: LanguageMode) {
  await persist({ language: value })
  if (typeof document !== 'undefined') {
    const systemLanguage = typeof navigator !== 'undefined' && navigator.language.toLowerCase().startsWith('en') ? 'en-US' : 'zh-CN'
    document.documentElement.lang = value === 'system' ? systemLanguage : value
  }
}

async function setFont(value: FontFamilyMode) {
  if (!value.trim()) return
  ui.applyFont(value)
  await persist({ fontFamily: value })
}

async function setEditorFont(value: FontFamilyMode) {
  if (!value.trim()) return
  ui.applyEditorFont(value)
  await persist({ editorFontFamily: value })
}

async function setFontSize(key: 'uiFontSize' | 'resultFontSize', value: number | null) {
  if (value == null) return
  const size = Math.min(24, Math.max(11, Math.round(value)))
  ui.applyFontSizes(
    key === 'uiFontSize' ? size : app.settings.uiFontSize,
    key === 'resultFontSize' ? size : app.settings.resultFontSize,
  )
  await persist({ [key]: size })
}

function fontCssValue(value: string) {
  if (value === 'system') return undefined
  return `'${value.replace(/["']/g, '')}', sans-serif`
}

function renderFontOption(option: { label?: string | number; value?: string | number }) {
  const value = String(option.value ?? '')
  const label = String(option.label ?? value)
  return h('span', { style: { fontFamily: fontCssValue(value) } }, `${label}  中文 Aa 123`)
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
        <div class="settings-scroll" :class="{ 'settings-scroll-shortcuts': activeSection === 'shortcuts' }">
          <AppAlert v-if="settingsError" class="settings-banner" type="error" :bordered="false">
            {{ settingsError }}
          </AppAlert>

          <template v-if="activeSection === 'appearance'">
            <section class="settings-group">
              <div class="group-heading">
                <h2>主题</h2>
                <p>选择应用的明暗模式。</p>
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
                  @click="setTheme(option.value as ThemeMode)"
                >
                  <span class="choice-title">{{ option.label }}</span>
                  <span class="choice-description">{{ option.value === 'dark' ? '适合低光环境' : option.value === 'light' ? '清晰明亮的界面' : '跟随系统设置' }}</span>
                </button>
              </div>
            </section>

            <section class="settings-group">
              <div class="setting-row">
                <div class="setting-copy"><h2>语言</h2><p>控制组件内置文案。</p></div>
                <AppSelect class="control" size="medium" :value="app.settings.language" :options="languageOptions" @update:value="(value: any) => setLanguage(value)" />
              </div>
            </section>

            <section class="settings-group">
              <div class="group-heading"><h2>排版</h2><p>调整界面文字和查询结果的阅读体验。</p></div>
              <div class="setting-row">
                <div class="setting-copy"><h3>字体</h3><p>从本机已安装的系统字体中选择。</p></div>
                <AppSelect width="620px" size="medium" :value="app.settings.fontFamily" :options="fontOptions" :render-label="renderFontOption" filterable tag clearable placeholder="选择字体" @update:value="(value: string | null) => setFont(value || 'system')" />
              </div>
              <div class="font-preview" :style="{ fontFamily: fontCssValue(app.settings.fontFamily) }">中文预览 Aa 123</div>
              <div class="setting-row">
                <div class="setting-copy"><h3>请求编辑器字体</h3><p>控制 URL、参数、请求 Headers 和 Body；不影响响应查看器。默认使用内置等宽字体。</p></div>
                <AppSelect width="620px" size="medium" :value="app.settings.editorFontFamily" :options="editorFontOptions" :render-label="renderFontOption" filterable tag clearable placeholder="选择请求编辑器字体" @update:value="(value: string | null) => setEditorFont(value || 'system')" />
              </div>
              <div class="font-preview" :style="{ fontFamily: 'var(--a-font-editor)' }">参数名：Authorization　值：Bearer token</div>
              <div class="setting-row">
                <div class="setting-copy"><h3>应用界面字号</h3><p>控制菜单、按钮和设置文字。</p></div>
                <AppInputNumber :value="app.settings.uiFontSize" :min="11" :max="24" :step="1" :show-button="true" @update:value="(value: number | null) => setFontSize('uiFontSize', value)">
                  <template #suffix>px</template>
                </AppInputNumber>
              </div>
              <div class="setting-row">
                <div class="setting-copy"><h3>查询结果字号</h3><p>控制响应正文、树视图和数据表。</p></div>
                <AppInputNumber :value="app.settings.resultFontSize" :min="11" :max="24" :step="1" :show-button="true" @update:value="(value: number | null) => setFontSize('resultFontSize', value)">
                  <template #suffix>px</template>
                </AppInputNumber>
              </div>
            </section>

            <section class="settings-group">
              <div class="setting-row">
                <div class="setting-copy"><h2>布局</h2><p>恢复工作区导航栏的默认宽度。</p></div>
                <AppButton secondary @click="ui.resetSidebarWidth()">
                  <template #icon><AppIcon :component="IconReset" :size="16" /></template>
                  恢复默认宽度
                </AppButton>
              </div>
            </section>
          </template>

          <template v-else-if="activeSection === 'startup'">
            <section class="settings-group">
              <div class="setting-row">
                <div class="setting-copy">
                  <h2>启动</h2>
                  <p>启动时自动进入最近一次可用的工作区。</p>
                </div>
                <AppSwitch :value="app.settings.openLastWorkspaceOnStart" @update:value="setAutoOpen" />
              </div>
            </section>

            <section class="settings-group">
              <div class="setting-row setting-row-top setting-row-long">
                <div class="setting-copy">
                  <h2>默认工作区位置</h2>
                  <p>新建工作区默认保存到此目录。</p>
                </div>
                <div class="path-control">
                  <AppInput
                    class="control-grow"
                    size="medium"
                    :value="app.settings.defaultWorkspaceRoot"
                    readonly
                    :input-props="{ spellcheck: false }"
                  />
                  <AppButton secondary size="medium" @click="chooseWorkspaceRoot">更改位置</AppButton>
                </div>
              </div>
            </section>
          </template>

          <template v-else-if="activeSection === 'network'">
            <section class="settings-group">
              <div class="group-heading">
                <h2>连接</h2>
                <p>当前代理：{{ proxyModeLabel }}。</p>
              </div>
              <div class="setting-row">
                <div class="setting-copy"><h3>代理</h3><p>选择请求使用的代理方式。</p></div>
                <AppSelect width="180px" size="medium" :value="app.settings.proxyMode" :options="proxyOptions" @update:value="(value: any) => setNetwork({ proxyMode: value })" />
              </div>
              <div v-if="app.settings.proxyMode === 'manual'" class="setting-row">
                <div class="setting-copy"><h3>代理地址</h3><p>HTTP 或 SOCKS 代理服务器地址。</p></div>
                <AppInput width="620px" size="medium" :value="proxyUrlDraft" placeholder="http://127.0.0.1:8080" :input-props="{ spellcheck: false }" @update:value="onProxyUrlInput" @blur="flushProxyUrl" />
              </div>
              <div class="setting-row">
                <div class="setting-copy"><h3>超时</h3><p>整次请求，包含连接和下载。</p></div>
                <div class="setting-inline">
                  <AppInputNumber :value="app.settings.timeoutMs" class="control-num" size="medium" :min="0" :step="1000" @update:value="(value: number | null) => setNetwork({ timeoutMs: value ?? 30000 })" />
                  <span class="unit">ms</span>
                </div>
              </div>
              <div class="setting-row">
                <div class="setting-copy"><h3>校验 TLS</h3><p>验证 HTTPS 证书链。</p></div>
                <AppSwitch :value="app.settings.verifyTls" @update:value="(value: boolean) => setNetwork({ verifyTls: value })" />
              </div>
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

          <template v-else-if="activeSection === 'shortcuts'">
            <section class="shortcut-page" aria-label="快捷键设置">
              <div class="shortcut-search">
                <AppInput
                  class="shortcut-search-input"
                  size="medium"
                  :value="shortcutSearch"
                  :placeholder="shortcutSearchBy === 'name' ? '搜索名称或说明' : '按下快捷键进行搜索'"
                  clearable
                  :input-props="{ spellcheck: false }"
                  @update:value="shortcutSearchBy === 'name' && (shortcutSearch = $event)"
                  @keydown="captureShortcutSearch"
                />
                <div class="shortcut-search-mode" role="group" aria-label="搜索方式">
                  <button type="button" :class="{ active: shortcutSearchBy === 'name' }" @click="setShortcutSearchMode('name')">名称</button>
                  <button type="button" :class="{ active: shortcutSearchBy === 'key' }" @click="setShortcutSearchMode('key')">快捷键</button>
                </div>
              </div>

              <div v-if="visibleShortcutGroups.length" class="shortcut-grid">
                <section v-for="group in visibleShortcutGroups" :key="group.area" class="shortcut-area">
                  <h2>{{ group.area }}</h2>
                  <div class="shortcut-list">
                    <div v-for="shortcut in group.items" :key="shortcut.id" class="shortcut-item">
                      <div class="shortcut-copy">
                        <strong>{{ shortcut.name }}</strong>
                        <span>{{ shortcut.description }}</span>
                      </div>
                      <div class="shortcut-actions">
                        <div class="shortcut-keys" :aria-label="`${shortcut.name}：${shortcut.keys.join(' + ')}`">
                          <kbd v-for="key in shortcut.keys" :key="key">{{ key }}</kbd>
                        </div>
                        <AppSwitch :value="ui.isShortcutEnabled(shortcut.id)" @update:value="ui.setShortcutEnabled(shortcut.id, $event)" />
                      </div>
                    </div>
                  </div>
                </section>
              </div>
              <p v-else class="shortcut-empty">没有找到匹配的快捷键。</p>
            </section>
          </template>

          <template v-else>
            <section class="settings-group">
              <div class="group-heading">
                <h2>应用</h2>
                <p>本地优先的桌面 API 客户端。</p>
              </div>
              <dl class="about-list settings-info-list">
                <div class="setting-row">
                  <dt>版本</dt>
                  <dd>v{{ APP_VERSION }}</dd>
                </div>
                <div class="setting-row">
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
  grid-template-columns: 164px minmax(0, 1fr);
  min-height: 0;
  overflow: hidden;
  background: var(--a-bg-2);
}

.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 18px 8px;
  border-right: 1px solid var(--a-border);
  background: var(--a-bg-3);
  width: 164px;
}
.nav-heading { padding: 0 10px 14px; }
.nav-title {
  color: var(--a-text);
  font-size: var(--a-fs-xl);
  font-weight: var(--a-fw-semibold);
  line-height: var(--a-lh-xl);
}
.nav-subtitle { margin-top: 2px; color: var(--a-text-faint); font-size: var(--a-fs-sm); line-height: var(--a-lh-sm); }
.settings-nav button {
  position: relative;
  min-height: max(34px, var(--a-h-control-sm));
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 0 12px;
  border: 0;
  border-radius: 6px;
  background: transparent;
  color: var(--a-text-dim);
  font-size: var(--a-fs-base);
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
  width: min(1040px, 100%);
  margin-inline: auto;
  display: flex;
  flex-direction: column;
  gap: 8px;
  overflow: auto;
  padding: 30px 48px 52px;
}
.settings-banner { margin-bottom: 4px; }
.settings-group {
  min-width: 0;
  padding: 18px 0;
  border: 0;
  border-bottom: 0;
  background: transparent;
  border-radius: 0;
  box-shadow: none;
}
.settings-group.muted .placeholder {
  margin: 4px 0 0;
  color: var(--a-text-faint);
  font-size: var(--a-fs-base);
  line-height: var(--a-lh);
}
.settings-group.muted { opacity: .68; }
.group-heading { margin-bottom: 14px; }
.group-heading h2 {
  margin: 0 0 5px;
  color: var(--a-text);
  font-size: var(--a-fs-md);
  line-height: var(--a-lh);
  font-weight: 500;
}
.group-heading p {
  margin: 0;
  color: var(--a-text-dim);
  font-size: var(--a-fs-sm);
  line-height: var(--a-lh-sm);
}
.setting-row { display: grid; grid-template-columns: minmax(210px, 1fr) auto; align-items: center; gap: 24px; min-height: max(44px, var(--a-h-control)); }
.setting-row + .setting-row { margin-top: 14px; }
.setting-row-top { align-items: start; }
.setting-row-long { grid-template-columns: minmax(210px, 1fr) minmax(0, 680px); }
.setting-copy { min-width: 0; }
.setting-copy h2, .setting-copy h3 { margin: 0; color: var(--a-text); font-size: var(--a-fs-base); line-height: var(--a-lh); font-weight: var(--a-fw-medium); }
.setting-copy h2 { font-size: var(--a-fs-md); }
.setting-copy p { margin: 2px 0 0; color: var(--a-text-dim); font-size: var(--a-fs-sm); line-height: var(--a-lh-sm); }
.control { width: min(328px, 100%); }
.control-wide { width: min(328px, 100%); }
.font-preview { width: min(620px, 100%); margin-top: 8px; color: var(--a-text-dim); font-size: var(--a-fs-base); line-height: var(--a-lh); }
.font-size-form { width: min(328px, 100%); }
.font-size-form :deep(.n-input-number) { width: 140px; }
.control-num { width: 140px; }
.path-control { width: 100%; display: flex; align-items: center; gap: 8px; min-width: 0; }
.control-grow {
  flex: 1 1 0;
  min-width: 0;
  width: auto !important;
}
.path-control :deep(.n-input) { width: 100%; }
.unit {
  color: var(--a-text-faint);
  font: var(--a-fs-xs) var(--a-font-mono);
}
.hint { color: var(--a-text-faint); font-size: var(--a-fs-sm); line-height: var(--a-lh-sm); }
.setting-inline {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
  color: var(--a-text-faint);
  font-size: var(--a-fs-sm);
  line-height: var(--a-lh-sm);
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
.settings-info-list { max-width: none; }
.settings-info-list > div { grid-template-columns: minmax(210px, 1fr) auto; gap: 24px; min-height: max(44px, var(--a-h-control)); padding: 0; border-top: 0; }
.settings-info-list > div + div { margin-top: 14px; }
.settings-info-list dt { color: var(--a-text); font-size: var(--a-fs-base); font-weight: var(--a-fw-medium); }
.settings-info-list dd { justify-self: start; color: var(--a-text-soft); }
.about-list > div {
  display: grid;
  grid-template-columns: 96px minmax(0, 1fr);
  gap: 12px;
  align-items: baseline;
  min-height: var(--a-h-control);
  padding: 8px 0;
  border-top: 1px solid var(--a-border);
}
.about-list > div:first-child { border-top: 0; }
.about-list dt {
  margin: 0;
  color: var(--a-text-dim);
  font-size: var(--a-fs-base);
}
.about-list dd {
  margin: 0;
  color: var(--a-text);
  font-size: var(--a-fs-base);
  font-family: var(--a-font-mono);
}
.about-list dd.plain { font-family: var(--a-font-ui); }
.settings-group :deep(.n-form-item:last-child) { margin-bottom: 0; }
.settings-group :deep(.n-form-item) { margin-bottom: 14px; }
.settings-group :deep(.n-form-item-label) {
  color: var(--a-text-soft);
  font-weight: var(--a-fw-medium);
  font-size: var(--a-fs-base);
}
.choice-grid, .font-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; max-width: none; }
.font-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
.choice-card { min-height: max(68px, calc(var(--a-lh) + var(--a-lh-sm) + 28px)); padding: 12px 14px; border: 1px solid var(--a-border-strong); border-radius: 6px; background: var(--a-bg-2); color: var(--a-text); text-align: left; cursor: pointer; }
.choice-card:hover { background: var(--a-hover); }
.choice-card.selected { background: var(--a-bg-3); border-color: var(--a-text-dim); }
.choice-title { display: block; font-size: var(--a-fs-base); font-weight: 500; line-height: var(--a-lh); }
.choice-description { display: block; margin-top: 4px; color: var(--a-text-dim); font-size: var(--a-fs-sm); line-height: var(--a-lh-sm); }
.settings-scroll-shortcuts { width: 100%; }
.shortcut-page { width: 100%; display: flex; flex-direction: column; gap: 28px; }
.shortcut-search { display: grid; grid-template-columns: minmax(280px, 1fr) auto; align-items: center; gap: 12px; padding-bottom: 20px; border-bottom: 1px solid var(--a-border); }
.shortcut-search-input { flex: 1 1 auto; min-width: 0; }
.shortcut-search-mode { display: inline-flex; flex: 0 0 auto; padding: 3px; border: 1px solid var(--a-border); border-radius: 6px; background: var(--a-bg-3); }
.shortcut-search-mode button { min-height: var(--a-h-control-xs); padding: 0 11px; border: 0; border-radius: 4px; background: transparent; color: var(--a-text-dim); font: var(--a-fs-sm)/var(--a-lh-sm) var(--a-font-ui); cursor: pointer; }
.shortcut-search-mode button:hover { color: var(--a-text); background: var(--a-hover); }
.shortcut-search-mode button.active { color: var(--a-text); background: var(--a-bg-2); box-shadow: inset 0 0 0 1px var(--a-border-strong); }
.shortcut-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(360px, 1fr)); gap: 30px 36px; align-items: start; }
.shortcut-area { min-width: 0; }
.shortcut-area h2 { margin: 0 0 10px; color: var(--a-text-soft); font-size: var(--a-fs-base); font-weight: var(--a-fw-semibold); line-height: var(--a-lh); }
.shortcut-list { border-top: 1px solid var(--a-border-strong); }
.shortcut-item { display: flex; align-items: center; justify-content: space-between; gap: 18px; min-height: max(78px, calc(var(--a-lh) + var(--a-lh-sm) + 32px)); padding: 14px 2px; border-bottom: 1px solid var(--a-border); }
.shortcut-item:hover { background: var(--a-hover); box-shadow: 10px 0 0 var(--a-hover), -10px 0 0 var(--a-hover); }
.shortcut-copy { min-width: 0; display: flex; flex-direction: column; gap: 4px; }
.shortcut-copy strong { color: var(--a-text); font-size: var(--a-fs-base); font-weight: var(--a-fw-medium); line-height: var(--a-lh); }
.shortcut-copy span { color: var(--a-text-dim); font-size: var(--a-fs-sm); line-height: var(--a-lh-sm); }
.shortcut-actions { display: flex; flex: 0 0 auto; align-items: center; gap: 16px; }
.shortcut-keys { display: flex; align-items: center; justify-content: flex-end; gap: 4px; }
.shortcut-keys kbd { display: inline-flex; align-items: center; justify-content: center; min-width: 24px; min-height: var(--a-h-control-xs); padding: 0 6px; border: 1px solid var(--a-border-strong); border-bottom-color: var(--a-text-faint); border-radius: 4px; background: var(--a-bg-3); color: var(--a-text-soft); font: var(--a-fs-xs)/var(--a-lh-xs) var(--a-font-mono); white-space: nowrap; }
.shortcut-empty { margin: 0; color: var(--a-text-faint); font-size: var(--a-fs-base); }
.settings-group :deep(.n-form-item) { margin-bottom: 12px; }
.settings-group :deep(.n-input), .settings-group :deep(.n-base-selection), .settings-group :deep(.n-button) { border-radius: 6px; }
.settings-group > .font-preview { margin-left: calc(min(210px, 100%) + 24px); }

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
  .settings-scroll { width: 100%; padding: 18px 16px 32px; }
  .setting-row { grid-template-columns: 1fr; gap: 8px; align-items: start; }
  .settings-group > .font-preview { margin-left: 0; }
  .control-wide, .path-control, .font-preview { width: 100%; }
  .path-control { flex-wrap: wrap; }
  .control-grow { flex: 1 1 100%; }
  .about-list > div { grid-template-columns: 1fr; gap: 2px; }
  .shortcut-search { grid-template-columns: 1fr; align-items: stretch; }
  .shortcut-search-mode { align-self: flex-start; }
  .shortcut-grid { grid-template-columns: 1fr; gap: 20px; }
}
</style>
