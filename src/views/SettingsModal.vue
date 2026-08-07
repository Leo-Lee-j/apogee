<script setup lang="ts">
import { ref } from 'vue'
import {
  AppAlert,
  AppButton,
  AppForm,
  AppFormItem,
  AppInputNumber,
  AppInput,
  AppSelect,
  AppSwitch,
} from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { AppIcon } from '@/components/ui'
import { IconFolderOpen, IconNetwork, IconPalette, IconReset, IconShield } from '@/components/ui/icons'
import { useUiStore } from '@/stores/ui'
import { useAppStore } from '@/stores/app'

type SettingsSection = 'appearance' | 'network' | 'privacy'

const ui = useUiStore()
const app = useAppStore()
const activeSection = ref<SettingsSection>('appearance')
const proxyMode = ref('system')
const timeout = ref(30000)
const verifyTls = ref(true)
const telemetry = ref(false)

const sections = [
  { id: 'appearance' as const, label: '外观与布局', icon: IconPalette },
  { id: 'network' as const, label: '网络', icon: IconNetwork },
  { id: 'privacy' as const, label: '隐私', icon: IconShield },
]

const themeOptions = [
  { label: '暗色', value: 'dark' },
  { label: '亮色', value: 'light' },
  { label: '跟随系统', value: 'system' },
]
const layoutOptions = [
  { label: '上下分栏', value: 'stacked' },
  { label: '左右分栏', value: 'side-by-side' },
  { label: '专注模式', value: 'focus' },
]

async function chooseWorkspaceRoot() {
  const { open } = await import('@tauri-apps/plugin-dialog')
  const selected = await open({ directory: true, multiple: false })
  if (typeof selected === 'string') {
    await app.saveSettings({ ...app.settings, defaultWorkspaceRoot: selected })
  }
}

async function setAutoOpen(value: boolean) {
  await app.saveSettings({ ...app.settings, openLastWorkspaceOnStart: value })
}
</script>

<template>
  <NaiveThemeScope>
    <section class="settings-page" aria-label="设置">
      <aside class="settings-nav" aria-label="设置分类">
        <div class="nav-heading">
          <div class="nav-title">设置</div>
          <div class="nav-subtitle">偏好设置</div>
        </div>
        <button
          v-for="section in sections"
          :key="section.id"
          type="button"
          :class="{ active: activeSection === section.id }"
          @click="activeSection = section.id"
        >
          <AppIcon :component="section.icon" :size="16" />
          <span>{{ section.label }}</span>
        </button>
      </aside>

      <div class="settings-content">
        <div class="settings-scroll">
          <template v-if="activeSection === 'appearance'">
            <section class="settings-group">
              <div class="group-heading">
                <h2>界面</h2>
                <p>控制应用主题和工作区排列方式。</p>
              </div>
              <AppForm label-placement="left" label-width="112" size="medium">
                <AppFormItem label="主题">
                  <AppSelect
                    class="control"
                    size="medium"
                    :value="ui.themeMode"
                    :options="themeOptions"
                    @update:value="(value: any) => { ui.themeMode = value; ui.applyTheme() }"
                  />
                </AppFormItem>
                <AppFormItem label="内容布局">
                  <AppSelect
                    class="control"
                    size="medium"
                    :value="ui.layout"
                    :options="layoutOptions"
                    @update:value="(value: any) => { ui.setLayout(value) }"
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

            <section class="settings-group">
              <div class="group-heading">
                <h2>启动</h2>
                <p>控制 Apogee 启动后首先显示的界面。</p>
              </div>
              <AppForm label-placement="left" label-width="190" size="medium">
                <AppFormItem label="启动时打开上次使用的工作区">
                  <AppSwitch
                    :value="app.settings.openLastWorkspaceOnStart"
                    @update:value="setAutoOpen"
                  />
                </AppFormItem>
              </AppForm>
            </section>

            <section class="settings-group">
              <div class="group-heading">
                <h2>工作区</h2>
                <p>新建工作区默认保存到此目录，已有工作区不会被移动。</p>
              </div>
              <div class="workspace-root-control">
                <span class="workspace-icon"><AppIcon :component="IconFolderOpen" :size="17" /></span>
                <AppInput :value="app.settings.defaultWorkspaceRoot" readonly />
                <AppButton secondary @click="chooseWorkspaceRoot">更改位置</AppButton>
              </div>
            </section>
          </template>

          <template v-else-if="activeSection === 'network'">
            <section class="settings-group">
              <div class="group-heading">
                <h2>连接</h2>
                <p>设置请求使用的代理、超时和证书校验策略。</p>
              </div>
              <AppForm label-placement="left" label-width="112" size="medium">
                <AppFormItem label="代理">
                  <AppSelect
                    v-model:value="proxyMode"
                    class="control"
                    size="medium"
                    :options="[
                      { label: '系统代理', value: 'system' },
                      { label: '手动配置', value: 'manual' },
                      { label: '不使用代理', value: 'none' },
                    ]"
                  />
                </AppFormItem>
                <AppFormItem label="超时">
                  <AppInputNumber v-model:value="timeout" class="control" :min="0" :step="1000" />
                  <span class="unit">ms</span>
                </AppFormItem>
                <AppFormItem label="校验 TLS">
                  <AppSwitch v-model:value="verifyTls" />
                </AppFormItem>
              </AppForm>
              <AppAlert v-if="!verifyTls" type="warning" :bordered="false">
                关闭 TLS 校验后，连接可能被中间人窃听。仅用于明确知道风险的测试环境。
              </AppAlert>
            </section>
          </template>

          <template v-else>
            <section class="settings-group">
              <div class="group-heading">
                <h2>使用数据</h2>
                <p>Apogee 默认不发送遥测数据。</p>
              </div>
              <AppForm label-placement="left" label-width="140" size="medium">
                <AppFormItem label="发送匿名使用数据">
                  <AppSwitch v-model:value="telemetry" />
                </AppFormItem>
              </AppForm>
              <AppAlert type="info" :bordered="false">
                此选项默认关闭；离线使用不依赖任何遥测服务。
              </AppAlert>
            </section>
          </template>
        </div>
      </div>
    </section>
  </NaiveThemeScope>
</template>

<style scoped>
.settings-page {
  flex: 1 1 auto;
  display: grid;
  grid-template-columns: 208px minmax(0, 1fr);
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-1);
  box-shadow: var(--a-shadow-sm);
}
.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 16px 10px;
  border-right: 1px solid var(--a-border);
  background: var(--a-sidebar, #f4f4f5);
}
.nav-heading { padding: 0 10px 12px; }
.nav-title {
  color: var(--a-text);
  font-size: 16px;
  font-weight: var(--a-fw-semibold);
  line-height: 24px;
}
.nav-subtitle { margin-top: 2px; color: var(--a-text-dim); font-size: 12px; line-height: 18px; }
.settings-nav button {
  height: 40px;
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
  background: #111827;
  color: #ffffff;
  box-shadow: 0 1px 2px rgba(17, 24, 39, 0.12);
}
.settings-content { display: flex; flex-direction: column; min-width: 0; min-height: 0; background: var(--a-bg-1); }
.settings-scroll {
  flex: 1 1 auto;
  min-height: 0;
  display: grid;
  grid-template-columns: repeat(2, minmax(280px, 1fr));
  align-content: start;
  gap: 10px;
  overflow: auto;
  padding: 18px;
}
.settings-group {
  min-width: 0;
  padding: 10px;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-2);
  box-shadow: var(--a-shadow-sm);
}
.settings-group.wide,
.settings-group:only-child { grid-column: 1 / -1; }
.group-heading { margin-bottom: 12px; }
.group-heading h2 { margin: 0 0 6px; color: var(--a-text); font-size: 14px; line-height: 20px; font-weight: var(--a-fw-semibold); }
.group-heading p { margin: 0; color: var(--a-text-dim); font-size: 13px; line-height: 20px; }
.control { width: min(320px, 100%); }
.workspace-root-control { display: flex; align-items: center; gap: 8px; max-width: 680px; }
.workspace-root-control :deep(.n-input) { min-width: 0; flex: 1; }
.workspace-icon {
  width: 40px;
  height: 40px;
  flex: 0 0 auto;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  background: var(--a-bg-3);
  color: var(--a-text-dim);
}
.unit { margin-left: 8px; color: var(--a-text-faint); font: var(--a-fs-xs) var(--a-font-mono); }
.settings-group :deep(.n-form-item:last-child) { margin-bottom: 0; }
.settings-group :deep(.n-form-item-label) { color: var(--a-text-soft); font-weight: var(--a-fw-medium); }
.settings-group :deep(.n-base-selection),
.settings-group :deep(.n-input) { box-shadow: none; }

@media (max-width: 760px) {
  .settings-page { grid-template-columns: 1fr; grid-template-rows: auto minmax(0, 1fr); }
  .settings-nav {
    flex-direction: row;
    overflow-x: auto;
    padding: 10px;
    border-right: 0;
    border-bottom: 1px solid var(--a-border);
  }
  .nav-heading { display: none; }
  .settings-nav button { flex: 0 0 auto; }
  .settings-scroll { grid-template-columns: 1fr; padding: 14px; }
  .settings-group { padding: 10px; }
  .workspace-root-control { flex-wrap: wrap; }
  .workspace-root-control :deep(.n-input) { flex-basis: calc(100% - 48px); }
}
</style>
