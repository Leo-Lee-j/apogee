import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { ipc, onUpdateProgress, type UpdateCheckResult, type UpdateProgress, type UpdateRelease } from '@/ipc'
import { useUpdateStore } from './update'

vi.mock('@/ipc', async importOriginal => ({ ...await importOriginal<typeof import('@/ipc')>(), onUpdateProgress: vi.fn() }))
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const release: UpdateRelease = { tag: 'v0.2.0', version: '0.2.0', notes: '<script>not HTML</script>',
  publishedAt: null, prerelease: false, filename: 'Apogee_0.2.0_x64-setup.exe', size: 100, sha256: 'a'.repeat(64) }
const available: UpdateCheckResult = { currentVersion: '0.1.0', latest: release, updateAvailable: true }
const downloaded = { tag: release.tag, filename: release.filename, path: 'C:/cache/updates/' + release.filename }
let report: (event: UpdateProgress) => void
let unlisten = vi.fn<() => void>()
beforeEach(() => {
  setActivePinia(createPinia())
  unlisten = vi.fn<() => void>()
  vi.mocked(onUpdateProgress).mockImplementation(async (_, handler) => { report = handler; return unlisten })
  vi.spyOn(ipc, 'updateCheck').mockResolvedValue(available)
  vi.spyOn(ipc, 'updateCurrentVersion').mockResolvedValue('0.1.1')
  vi.spyOn(ipc, 'updateDownload').mockResolvedValue(downloaded)
  vi.spyOn(ipc, 'updateCancel').mockResolvedValue(true)
  vi.spyOn(ipc, 'updateOpenDownload').mockResolvedValue()
})
afterEach(() => { vi.restoreAllMocks(); vi.mocked(onUpdateProgress).mockReset() })

describe('application update lifecycle', () => {
  it('loads the real host version once', async () => {
    const store = useUpdateStore()
    await Promise.all([store.loadVersion(), store.loadVersion()])
    expect(store.currentVersion).toBe('0.1.1')
    expect(ipc.updateCurrentVersion).toHaveBeenCalledTimes(1)
  })
  it('checks stable releases by default and channel changes invalidate prior candidates', async () => {
    const store = useUpdateStore()
    await store.check()
    expect(ipc.updateCheck).toHaveBeenCalledWith(false)
    expect(store.phase).toBe('available')
    expect(store.canDownload).toBe(true)
    store.setChannel(true)
    expect(store.latest).toBeNull()
    expect(store.canDownload).toBe(false)
    await store.check()
    expect(ipc.updateCheck).toHaveBeenLastCalledWith(true)
  })
  it('distinguishes no published release from an up-to-date installed version', async () => {
    const store = useUpdateStore()
    vi.mocked(ipc.updateCheck).mockResolvedValueOnce({ currentVersion: '0.1.0', latest: null, updateAvailable: false })
    await store.check(); expect(store.phase).toBe('empty')
    vi.mocked(ipc.updateCheck).mockResolvedValueOnce({ ...available, updateAvailable: false })
    await store.check(); expect(store.phase).toBe('upToDate'); expect(store.canDownload).toBe(false)
  })
  it('serializes actions and cannot change channel during a check', async () => {
    const pending = deferred<UpdateCheckResult>()
    vi.mocked(ipc.updateCheck).mockReturnValue(pending.promise)
    const store = useUpdateStore()
    const checking = store.check()
    store.setChannel(true)
    await store.check()
    expect(store.includePrerelease).toBe(false)
    expect(ipc.updateCheck).toHaveBeenCalledTimes(1)
    pending.resolve(available); await checking
  })
  it('clears an old candidate when a new check fails', async () => {
    const store = useUpdateStore(); await store.check()
    vi.mocked(ipc.updateCheck).mockRejectedValueOnce(new Error('rate limit'))
    await store.check()
    expect(store.phase).toBe('failed'); expect(store.latest).toBeNull()
    expect(store.error).toBe('rate limit'); expect(store.canDownload).toBe(false)
  })
  it('subscribes before downloading and isolates monotonic progress and cleanup', async () => {
    const pending = deferred<typeof downloaded>()
    vi.mocked(ipc.updateDownload).mockReturnValue(pending.promise)
    const store = useUpdateStore(); await store.check()
    const downloading = store.download(); await Promise.resolve()
    const id = vi.mocked(ipc.updateDownload).mock.calls[0]![0]
    expect(onUpdateProgress).toHaveBeenCalledWith(id, expect.any(Function))
    expect(vi.mocked(onUpdateProgress).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(ipc.updateDownload).mock.invocationCallOrder[0]!)
    report({ operationId: 'other', downloaded: 90, total: 900 }); expect(store.receivedBytes).toBe(0)
    report({ operationId: id, downloaded: 50, total: 900 }); expect(store.progress).toBe(50)
    report({ operationId: id, downloaded: 20, total: 100 }); expect(store.receivedBytes).toBe(50)
    await store.download(); expect(ipc.updateDownload).toHaveBeenCalledTimes(1)
    store.setChannel(true); expect(store.includePrerelease).toBe(false)
    pending.resolve(downloaded); await downloading
    expect(store.phase).toBe('ready'); expect(store.progress).toBe(100)
    expect(unlisten).toHaveBeenCalledTimes(1)
    report({ operationId: id, downloaded: 1, total: 100 }); expect(store.progress).toBe(100)
    expect(ipc.updateOpenDownload).not.toHaveBeenCalled()
  })
  it('disables early cancellation until backend registration and treats cancellation as retryable', async () => {
    const pending = deferred<typeof downloaded>()
    vi.mocked(ipc.updateDownload).mockReturnValue(pending.promise)
    const store = useUpdateStore(); await store.check()
    const downloading = store.download(); await Promise.resolve()
    await store.cancel(); expect(ipc.updateCancel).not.toHaveBeenCalled()
    const id = vi.mocked(ipc.updateDownload).mock.calls[0]![0]
    report({ operationId: id, downloaded: 0, total: 100 })
    await store.cancel(); expect(ipc.updateCancel).toHaveBeenCalledWith(id)
    expect(store.phase).toBe('cancelling')
    pending.reject({ code: 'UPDATE_CANCELLED', kind: 'io', message: '下载已取消', hints: [] }); await downloading
    expect(store.phase).toBe('cancelled'); expect(store.error).toBeNull(); expect(store.canDownload).toBe(true)
    expect(unlisten).toHaveBeenCalledTimes(1)
  })
  it('keeps completion if a cancellation arrives after a verified download', async () => {
    const pending = deferred<typeof downloaded>()
    vi.mocked(ipc.updateDownload).mockReturnValue(pending.promise)
    const store = useUpdateStore(); await store.check()
    const downloading = store.download(); await Promise.resolve()
    const id = vi.mocked(ipc.updateDownload).mock.calls[0]![0]
    report({ operationId: id, downloaded: 100, total: 100 })
    await store.cancel(); pending.resolve(downloaded); await downloading
    expect(store.phase).toBe('ready')
  })
  it('reports checksum/network failures without exposing an install action', async () => {
    const store = useUpdateStore(); await store.check()
    vi.mocked(ipc.updateDownload).mockRejectedValueOnce(new Error('checksum failed'))
    await store.download()
    expect(store.phase).toBe('failed'); expect(store.downloaded).toBeNull(); expect(store.canDownload).toBe(true)
    expect(unlisten).toHaveBeenCalledTimes(1)
    await store.openDownloaded(false); expect(ipc.updateOpenDownload).not.toHaveBeenCalled()
  })
  it('never starts a download if the progress subscription failed', async () => {
    vi.mocked(onUpdateProgress).mockRejectedValueOnce(new Error('listener unavailable'))
    const store = useUpdateStore(); await store.check(); await store.download()
    expect(store.phase).toBe('failed'); expect(ipc.updateDownload).not.toHaveBeenCalled()
  })
  it('opens only the backend-held tag, never a user-provided path, and never exits automatically', async () => {
    const store = useUpdateStore(); await store.check(); await store.download()
    await store.openDownloaded(true); expect(ipc.updateOpenDownload).toHaveBeenCalledWith(release.tag, true)
    expect(store.installStarted).toBe(false)
    await store.openDownloaded(false); expect(ipc.updateOpenDownload).toHaveBeenCalledWith(release.tag, false)
    expect(store.installStarted).toBe(true)
    vi.mocked(ipc.updateOpenDownload).mockRejectedValueOnce(new Error('changed file'))
    await store.openDownloaded(false); expect(store.error).toBe('changed file'); expect(store.opening).toBe(false)
  })
  it('offers re-download when the cached installer was deleted or modified before opening', async () => {
    const store = useUpdateStore(); await store.check(); await store.download()
    vi.mocked(ipc.updateOpenDownload).mockRejectedValueOnce({ code: 'UPDATE_PACKAGE_INVALID', kind: 'io', message: '安装包校验失败', hints: [] })
    await store.openDownloaded(false)
    expect(store.downloaded).toBeNull(); expect(store.phase).toBe('available')
    expect(store.canDownload).toBe(true); expect(store.error).toBe('安装包校验失败')
  })

})
