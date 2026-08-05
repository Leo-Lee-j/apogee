<script setup lang="ts">
import { ref } from 'vue'
import {
  AppAlert,
  AppButton,
  AppCard,
  AppForm,
  AppFormItem,
  AppInputNumber,
  AppModal,
  AppSelect,
  AppSwitch,
  AppTabPane,
  AppTabs,
} from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { useUiStore } from '@/stores/ui'

/**
 * 设置页。**这是 naive-ui 的正确用法**（02 文档 §7.5 第 5 条）：
 * 低频、非热路径、表单密集。
 *
 * 而且整个文件走动态 import —— 用户不打开设置就永远不会下载 naive-ui 的
 * 表单组件代码。这是 Spike S5 变体 B 的核心手法。
 */

const ui = useUiStore()

const themeOptions = [
  { label: '暗色', value: 'dark' },
  { label: '亮色', value: 'light' },
  { label: '跟随系统', value: 'system' },
]
const proxyMode = ref('system')
const timeout = ref(30000)
const verifyTls = ref(true)
const telemetry = ref(false)
</script>

<template>
  <NaiveThemeScope>
    <AppModal v-model:show="ui.settingsOpen">
    <AppCard style="width: 640px" title="设置" :bordered="false" size="small" role="dialog">
      <AppTabs type="line" animated>
        <AppTabPane name="appearance" tab="外观">
          <AppForm label-placement="left" label-width="96" size="small">
            <AppFormItem label="主题">
              <AppSelect
                :value="ui.themeMode"
                :options="themeOptions"
                @update:value="(v: any) => { ui.themeMode = v; ui.applyTheme() }"
              />
            </AppFormItem>
            <AppFormItem label="布局">
              <AppButton size="small" @click="ui.cycleLayout()">
                {{ ui.layout }}（Ctrl+Shift+L 循环）
              </AppButton>
            </AppFormItem>
          </AppForm>
        </AppTabPane>

        <AppTabPane name="network" tab="网络">
          <AppForm label-placement="left" label-width="96" size="small">
            <AppFormItem label="代理">
              <AppSelect
                v-model:value="proxyMode"
                :options="[
                  { label: '系统代理', value: 'system' },
                  { label: '手动', value: 'manual' },
                  { label: '不使用', value: 'none' },
                ]"
              />
            </AppFormItem>
            <AppFormItem label="超时 (ms)">
              <AppInputNumber v-model:value="timeout" :min="0" :step="1000" />
            </AppFormItem>
            <AppFormItem label="校验 TLS">
              <AppSwitch v-model:value="verifyTls" />
            </AppFormItem>
          </AppForm>
          <AppAlert v-if="!verifyTls" type="warning" :bordered="false" style="margin-top: 8px">
            关闭 TLS 校验后，连接可被中间人窃听。仅在明确知道风险的测试环境使用。
          </AppAlert>
        </AppTabPane>

        <AppTabPane name="privacy" tab="隐私">
          <AppForm label-placement="left" label-width="140" size="small">
            <AppFormItem label="发送使用数据">
              <AppSwitch v-model:value="telemetry" />
            </AppFormItem>
          </AppForm>
          <AppAlert type="info" :bordered="false">
            默认零遥测，默认关闭且需显式开启（PRD §10.2）。
            产品闭源（决策 D6），所以这条承诺靠「网络行为清单 + 断网可用」自证，
            而不是靠你相信我们 —— 见 PRD §9.3。
          </AppAlert>
        </AppTabPane>
      </AppTabs>
    </AppCard>
    </AppModal>
  </NaiveThemeScope>
</template>
