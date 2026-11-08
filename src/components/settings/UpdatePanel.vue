<script setup lang="ts">
import { onMounted } from 'vue'
import { AppAlert, AppButton, AppPopconfirm, AppSwitch } from '@/components/ui'
import { inTauri } from '@/ipc'
import { useUpdateStore } from '@/stores/update'

const updates = useUpdateStore()
onMounted(() => { void updates.loadVersion() })
function size(bytes: number) { return `${(bytes / 1024 / 1024).toFixed(1)} MiB` }
function published(value: string | null) {
  if (!value) return '未提供发布时间'
  const date = new Date(value)
  return Number.isNaN(date.valueOf()) ? '未提供发布时间' : date.toLocaleString()
}
</script>

<template>
  <section class="update-panel" aria-labelledby="update-heading">
    <div class="update-heading">
      <div>
        <h2 id="update-heading">软件更新</h2>
        <p>来源：GitHub · Leo-Lee-j/apogee · Windows x64</p>
      </div>
      <AppButton :loading="updates.phase === 'checking'" :disabled="!inTauri || updates.busy" @click="updates.check">检查更新</AppButton>
    </div>
    <label class="update-channel">
      <span>包含预发布版本 <small>默认只检查稳定版；预发布版可能不够稳定。</small></span>
      <AppSwitch :value="updates.includePrerelease" :disabled="!inTauri || updates.busy" aria-label="包含预发布版本" @update:value="updates.setChannel" />
    </label>
    <p v-if="!inTauri" class="update-status">请在 Apogee 桌面应用中检查和下载更新。</p>
    <p v-else class="update-status" role="status">{{ updates.status }}</p>
    <AppAlert v-if="updates.error" type="error" :show-icon="false">{{ updates.error }}</AppAlert>
    <div v-if="updates.latest" class="release-details">
      <div class="release-title">
        <strong>v{{ updates.latest.version }}</strong>
        <span>{{ updates.latest.prerelease ? '预发布版' : '稳定版' }}</span>
        <span>{{ published(updates.latest.publishedAt) }}</span>
      </div>
      <p>{{ updates.latest.filename }} · {{ size(updates.latest.size) }}</p>
      <details v-if="updates.latest.notes">
        <summary>更新说明</summary>
        <div class="release-notes">{{ updates.latest.notes }}</div>
      </details>
      <div v-if="['starting', 'downloading', 'cancelling'].includes(updates.phase)" class="download-progress">
        <progress :value="updates.receivedBytes" :max="updates.totalBytes" aria-label="安装包下载进度" />
        <span>{{ updates.progress }}% · {{ size(updates.receivedBytes) }} / {{ size(updates.totalBytes) }}</span>
      </div>
      <div class="update-actions">
        <AppButton v-if="updates.canDownload" type="primary" @click="updates.download">{{ updates.phase === 'failed' || updates.phase === 'cancelled' ? '重新下载' : '下载更新' }}</AppButton>
        <AppButton v-if="updates.phase === 'downloading' || updates.phase === 'cancelling'" :disabled="updates.phase === 'cancelling'" @click="updates.cancel">取消下载</AppButton>
        <template v-if="updates.phase === 'ready' && updates.downloaded">
          <AppPopconfirm positive-text="打开安装包" negative-text="暂不安装" :disabled="updates.busy" @positive-click="updates.openDownloaded(false)">
            <template #trigger><AppButton type="primary" :loading="updates.opening" :disabled="updates.busy">打开安装包</AppButton></template>
            请先保存未保存的请求。安装程序启动后，请手动退出 Apogee 再完成安装；应用不会自动关闭。
          </AppPopconfirm>
          <AppButton :disabled="updates.busy" @click="updates.openDownloaded(true)">在文件夹中显示</AppButton>
        </template>
      </div>
      <p v-if="updates.downloaded" class="download-path">缓存位置：{{ updates.downloaded.path }}</p>
    </div>
    <p class="update-footnote">仅下载已发布标签对应的官方安装包，下载后校验大小与 SHA-256。校验不等于发布者签名；未签名安装包可能触发 Windows 安全提示。不会降级、自动安装或上传工作区数据。</p>
  </section>
</template>

<style scoped>
.update-panel { display: grid; gap: 14px; margin-top: 24px; padding-top: 24px; border-top: 1px solid var(--a-border); }
.update-heading { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
h2 { margin: 0; font-size: var(--a-fs-lg); color: var(--a-text); }
p { margin: 5px 0 0; color: var(--a-text-dim); font-size: var(--a-fs-sm); line-height: 1.6; }
.update-channel { display: flex; align-items: center; justify-content: space-between; gap: 16px; color: var(--a-text); }
.update-channel small { display: block; margin-top: 4px; color: var(--a-text-dim); }
.update-status { color: var(--a-text); }
.release-details { display: grid; gap: 10px; padding: 16px; border: 1px solid var(--a-border); border-radius: var(--a-radius-md); background: var(--a-bg-3); }
.release-title { display: flex; align-items: baseline; flex-wrap: wrap; gap: 12px; color: var(--a-text-dim); font-size: var(--a-fs-sm); }
.release-title strong { color: var(--a-text); font-size: var(--a-fs-md); }
summary { cursor: pointer; color: var(--a-text); }
.release-notes { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 280px; overflow-y: auto; margin-top: 10px; color: var(--a-text-dim); font-size: var(--a-fs-sm); line-height: 1.7; }
.download-progress { display: grid; gap: 6px; color: var(--a-text-dim); font-size: var(--a-fs-sm); }
progress { width: 100%; height: 8px; accent-color: var(--a-primary); }
.update-actions { display: flex; flex-wrap: wrap; gap: 8px; }
.download-path, .update-footnote { overflow-wrap: anywhere; }
</style>
