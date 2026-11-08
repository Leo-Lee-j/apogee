import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { version as packageVersion } from '../../package.json'
import { ipc, normalizeError, onUpdateProgress, type DownloadedUpdate, type UpdateRelease } from '@/ipc'

export type UpdatePhase = 'idle' | 'checking' | 'available' | 'upToDate' | 'empty' | 'starting' | 'downloading' | 'cancelling' | 'ready' | 'failed' | 'cancelled'

/** Application-scoped: switching settings tabs never loses or cancels a download. */
export const useUpdateStore = defineStore('update', () => {
  const currentVersion = ref(packageVersion)
  const includePrerelease = ref(false)
  const phase = ref<UpdatePhase>('idle')
  const latest = ref<UpdateRelease | null>(null)
  const updateAvailable = ref(false)
  const downloaded = ref<DownloadedUpdate | null>(null)
  const receivedBytes = ref(0)
  const totalBytes = ref(0)
  const error = ref<string | null>(null)
  const opening = ref(false)
  const installStarted = ref(false)
  let operationId: string | null = null
  let versionPromise: Promise<void> | null = null
  const busy = computed(() => opening.value || ['checking', 'starting', 'downloading', 'cancelling'].includes(phase.value))
  const canDownload = computed(() => updateAvailable.value && latest.value !== null && !busy.value && phase.value !== 'ready')
  const progress = computed(() => totalBytes.value > 0 ? Math.min(100, Math.floor(receivedBytes.value / totalBytes.value * 100)) : 0)
  const status = computed(() => ({
    idle: '手动检查 GitHub 发布标签，不会自动下载或关闭应用。',
    checking: '正在检查 GitHub 发布标签…',
    available: `发现新版本 v${latest.value?.version ?? ''}`,
    upToDate: '当前已是此渠道的最新版本。',
    empty: '此渠道还没有已发布的版本。',
    starting: '正在准备下载…',
    downloading: '正在下载安装包…',
    cancelling: '正在取消下载…',
    ready: installStarted.value ? '安装程序已启动。请保存工作后手动退出 Apogee，再完成安装。' : '下载完成，大小与 SHA-256 校验通过。',
    failed: '更新操作失败，可重试。',
    cancelled: '下载已取消，可重新下载。',
  }[phase.value]))

  function loadVersion(): Promise<void> {
    if (!versionPromise) {
      versionPromise = ipc.updateCurrentVersion().then(value => { currentVersion.value = value }).catch(() => { versionPromise = null })
    }
    return versionPromise
  }
  function setChannel(value: boolean) {
    if (busy.value || value === includePrerelease.value) return
    includePrerelease.value = value
    phase.value = 'idle'
    latest.value = null
    downloaded.value = null
    updateAvailable.value = false
    error.value = null
    installStarted.value = false
  }
  async function check() {
    if (busy.value) return
    phase.value = 'checking'
    error.value = null
    latest.value = null
    downloaded.value = null
    installStarted.value = false
    updateAvailable.value = false
    try {
      const result = await ipc.updateCheck(includePrerelease.value)
      currentVersion.value = result.currentVersion
      latest.value = result.latest
      updateAvailable.value = result.updateAvailable
      phase.value = !result.latest ? 'empty' : result.updateAvailable ? 'available' : 'upToDate'
    } catch (cause) {
      error.value = normalizeError(cause).message
      phase.value = 'failed'
    }
  }
  async function download() {
    if (!canDownload.value || !latest.value) return
    const candidate = latest.value
    const id = crypto.randomUUID()
    operationId = id
    phase.value = 'starting'
    error.value = null
    receivedBytes.value = 0
    totalBytes.value = candidate.size
    installStarted.value = false
    let unlisten: (() => void) | undefined
    try {
      unlisten = await onUpdateProgress(id, event => {
        if (operationId !== id || event.operationId !== id) return
        receivedBytes.value = Math.max(receivedBytes.value, Math.min(candidate.size, event.downloaded))
        // Totals come from checked metadata, not a potentially stale progress event.
        if (phase.value === 'starting') phase.value = 'downloading'
      })
      const result = await ipc.updateDownload(id, candidate.tag)
      downloaded.value = result
      receivedBytes.value = candidate.size
      phase.value = 'ready'
    } catch (cause) {
      const failure = normalizeError(cause)
      if (failure.code === 'UPDATE_CANCELLED' || failure.message.startsWith('UPDATE_CANCELLED:')) {
        phase.value = 'cancelled'
      } else {
        error.value = failure.message
        phase.value = 'failed'
      }
    } finally {
      unlisten?.()
      if (operationId === id) operationId = null
    }
  }
  async function cancel() {
    if (phase.value !== 'downloading' || !operationId) return
    const id = operationId
    phase.value = 'cancelling'
    try { await ipc.updateCancel(id) } catch (cause) {
      if (operationId === id && phase.value === 'cancelling') {
        phase.value = 'downloading'
        error.value = normalizeError(cause).message
      }
    }
    // The download's terminal result decides whether cancellation or completion won.
  }
  async function openDownloaded(reveal: boolean) {
    if (busy.value || phase.value !== 'ready' || !downloaded.value) return
    opening.value = true
    error.value = null
    try {
      await ipc.updateOpenDownload(downloaded.value.tag, reveal)
      if (!reveal) installStarted.value = true
    } catch (cause) {
      const failure = normalizeError(cause)
      error.value = failure.message
      if (failure.code === 'UPDATE_PACKAGE_INVALID') {
        downloaded.value = null
        installStarted.value = false
        phase.value = 'available'
      }
    } finally { opening.value = false }
  }
  return { currentVersion, includePrerelease, phase, latest, downloaded, receivedBytes, totalBytes, error,
    opening, installStarted, busy, canDownload, updateAvailable, progress, status,
    loadVersion, setChannel, check, download, cancel, openDownloaded }
})
