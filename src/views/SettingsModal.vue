<script setup lang="ts">
import { computed, ref } from 'vue'
import {
  AppAlert,
  AppButton,
  AppForm,
  AppFormItem,
  AppInputNumber,
  AppSelect,
  AppSwitch,
} from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { useUiStore } from '@/stores/ui'

type SettingsSection = 'appearance' | 'network' | 'privacy'

const ui = useUiStore()
const activeSection = ref<SettingsSection>('appearance')
const proxyMode = ref('system')
const timeout = ref(30000)
const verifyTls = ref(true)
const telemetry = ref(false)

const sections: Array<{ id: SettingsSection; label: string }> = [
  { id: 'appearance', label: '外观与布局' },
  { id: 'network', label: '网络' },
  { id: 'privacy', label: '隐私' },
]

const sectionTitle = computed(() =>
  sections.find((section) => section.id === activeSection.value)?.label ?? '设置',
)
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
</script>

<template>
  <NaiveThemeScope>
    <section class="settings-page a-block" aria-labelledby="settings-title">
      <aside class="settings-nav" aria-label="设置分类">
        <div class="nav-title">设置</div>
        <button
          v-for="section in sections"
          :key="section.id"
          type="button"
          :class="{ active: activeSection === section.id }"
          @click="activeSection = section.id"
        >
          {{ section.label }}
        </button>
      </aside>

      <div class="settings-content">
        <header class="settings-header">
          <h1 id="settings-title">{{ sectionTitle }}</h1>
        </header>

        <div class="settings-scroll">
          <template v-if="activeSection === 'appearance'">
            <section class="settings-group">
              <div class="group-heading">
                <h2>界面</h2>
                <p>控制应用主题和工作区排列方式。</p>
              </div>
              <AppForm label-placement="left" label-width="112" size="small">
                <AppFormItem label="主题">
                  <AppSelect
                    class="control"
                    :value="ui.themeMode"
                    :options="themeOptions"
                    @update:value="(value: any) => { ui.themeMode = value; ui.applyTheme() }"
                  />
                </AppFormItem>
                <AppFormItem label="内容布局">
                  <AppSelect
                    class="control"
                    :value="ui.layout"
                    :options="layoutOptions"
                    @update:value="(value: any) => { ui.layout = value }"
                  />
                </AppFormItem>
              </AppForm>
            </section>

            <section class="settings-group">
              <div class="group-heading">
                <h2>侧边栏</h2>
                <p>恢复工作区导航栏的默认宽度。</p>
              </div>
              <AppButton size="small" @click="ui.resetSidebarWidth()">恢复默认宽度</AppButton>
            </section>
          </template>

          <template v-else-if="activeSection === 'network'">
            <section class="settings-group">
              <div class="group-heading">
                <h2>连接</h2>
                <p>设置请求使用的代理、超时和证书校验策略。</p>
              </div>
              <AppForm label-placement="left" label-width="112" size="small">
                <AppFormItem label="代理">
                  <AppSelect
                    v-model:value="proxyMode"
                    class="control"
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
              <AppForm label-placement="left" label-width="140" size="small">
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
  grid-template-columns: 190px minmax(0, 1fr);
  min-height: 0;
}
.settings-nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 12px 8px;
  border-right: 1px solid var(--a-border);
  background: var(--a-bg-2);
}
.nav-title {
  height: 30px;
  padding: 0 10px;
  color: var(--a-text);
  font-weight: var(--a-fw-semibold);
  line-height: 30px;
}
.settings-nav button {
  height: 30px;
  padding: 0 10px;
  border: 0;
  border-radius: var(--a-radius-sm);
  background: transparent;
  color: var(--a-text-dim);
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.settings-nav button:hover { background: var(--a-hover); color: var(--a-text); }
.settings-nav button.active {
  background: var(--a-selected);
  color: var(--a-text);
  box-shadow: inset 2px 0 0 var(--a-primary);
}
.settings-content { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
.settings-header {
  flex: 0 0 auto;
  height: 56px;
  display: flex;
  align-items: center;
  padding: 0 24px;
  border-bottom: 1px solid var(--a-border);
}
.settings-header h1 { margin: 0; font-size: 16px; line-height: 1.3; font-weight: var(--a-fw-semibold); }
.settings-scroll { flex: 1 1 auto; min-height: 0; overflow: auto; padding: 24px; }
.settings-group { width: min(680px, 100%); padding-bottom: 24px; }
.settings-group + .settings-group { padding-top: 24px; border-top: 1px solid var(--a-border); }
.group-heading { margin-bottom: 18px; }
.group-heading h2 { margin: 0 0 4px; color: var(--a-text); font-size: 14px; line-height: 20px; }
.group-heading p { margin: 0; color: var(--a-text-dim); font-size: 12px; line-height: 18px; }
.control { width: min(320px, 100%); }
.unit { margin-left: 8px; color: var(--a-text-faint); font: var(--a-fs-xs) var(--a-font-mono); }

@media (max-width: 760px) {
  .settings-page { grid-template-columns: 150px minmax(0, 1fr); }
  .settings-scroll { padding: 18px; }
  .settings-header { padding: 0 18px; }
}
</style>
