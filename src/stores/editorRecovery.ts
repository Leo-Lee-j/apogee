import { ref, shallowRef, watch, type WatchStopHandle } from 'vue'
import { defineStore } from 'pinia'
import { ipc, inTauri, normalizeError, type AppError, type AuthConfig, type Body, type ContainerDraft, type RequestDraft, type RecoverySnapshot, type WorkspaceContainer } from '@/ipc'
import { useTabsStore, type Tab } from './tabs'
import { bodyDraftBytes, BODY_DRAFT_BYTES, type BodyDrafts } from '@/features/bodyDrafts'

interface ContainerEditor extends ContainerDraft { dirty: boolean }

export const useEditorRecoveryStore = defineStore('editor-recovery', () => {
  const containers = shallowRef<Record<string, ContainerEditor>>({})
  const lastError = ref<AppError | null>(null)
  let archived: RequestDraft[] = []
  const excludedDrafts = new Set<string>()
  let stopWatch: WatchStopHandle | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  let generation = 0
  let revision = 0
  let pending: Promise<boolean> | undefined
  let started = false

  function draft(tab: Tab): RequestDraft {
    const { method, url, params, pathParams, headers, body, auth, settings } = tab
    return { id: tab.recoveryId, title: tab.title, dirty: tab.dirty, bodyDrafts: tab.bodyDrafts, spec: { method, url, params, pathParams, headers, body, auth, settings },
      nodeId: tab.nodeId, expectedAuth: tab.persistedAuth, sourceRequestId: tab.sourceRequestId,
      debugCaseId: tab.debugCaseId, debugCaseEditor: tab.debugCaseEditor ?? false }
  }
  function snapshot(): RecoverySnapshot {
    return {
      requests: [...archived.filter(item => !useTabsStore().tabs.some(tab => tab.recoveryId === item.id || (item.nodeId && item.nodeId === tab.nodeId))), ...useTabsStore().tabs.filter(tab => tab.kind === 'request' && (tab.dirty || Object.keys(tab.bodyDrafts).length > 0) && !tab.historyPreview && !excludedDrafts.has(tab.recoveryId)).map(draft)],
      containers: Object.values(containers.value).filter(entry => entry.dirty).map(({ value, expectedAuth }) => ({ value, expectedAuth })),
    }
  }
  function schedule() {
    revision++
    if (!started || !inTauri) return
    if (timer) clearTimeout(timer)
    timer = setTimeout(() => {
      timer = undefined
      const version = generation
      void flush().then(async ok => {
        if (ok || version !== generation || !lastError.value) return
        const { toast, errorText } = await import('@/features/feedback')
        if (version === generation && lastError.value) toast().error(errorText(lastError.value.message, lastError.value.hints))
      })
    }, 300)
  }
  function stop() {
    generation++
    started = false
    stopWatch?.(); stopWatch = undefined
    if (timer) clearTimeout(timer)
    timer = undefined; pending = undefined; archived = []
    excludedDrafts.clear()
    containerSaves.clear()
    containers.value = {}; lastError.value = null
  }
  async function start() {
    stop()
    const version = generation
    const saved = inTauri ? await ipc.workspaceReadRecovery() : { requests: [], containers: [] }
    if (version !== generation) return
    const tabs = useTabsStore()
    for (const item of saved.requests) {
      // Clean saved requests restore only their inactive types, when opened from disk.
      // This avoids stale credentials/names and a startup fan-out of per-request IPC.
      if (item.dirty === false && item.nodeId) {
        archived.push(item)
        continue
      }
      tabs.open({ ...item.spec, title: item.title, recoveryId: item.id, nodeId: item.nodeId ?? undefined,
        persistedAuth: item.expectedAuth ?? undefined, sourceRequestId: item.sourceRequestId ?? undefined,
        debugCaseId: item.debugCaseId ?? undefined, debugCaseEditor: item.debugCaseEditor,
        dirty: item.dirty ?? true, bodyDrafts: item.bodyDrafts ?? {}, preview: false })
    }
    containers.value = Object.fromEntries(saved.containers.map(item => [item.value.id, { ...item, dirty: true }]))
    for (const item of saved.containers) tabs.openContainerConfig(item.value)
    started = true
    stopWatch = watch([() => tabs.tabs, containers], schedule, { flush: 'sync' })
  }
  async function flush(): Promise<boolean> {
    if (timer) clearTimeout(timer)
    timer = undefined
    if (!started || !inTauri) return true
    if (pending) return pending
    const version = generation
    const task = (async () => {
      try {
        let written: number
        do {
          written = revision
          await ipc.workspaceSaveRecovery(snapshot())
          if (version !== generation) return false
        } while (written !== revision)
        lastError.value = null
        return true
      } catch (error) { if (version === generation) lastError.value = normalizeError(error); return false }
    })()
    pending = task
    void task.then(() => { if (pending === task) pending = undefined })
    return task
  }
  function put(value: WorkspaceContainer) {
    if (!containers.value[value.id]?.dirty) {
      const next = { ...containers.value }
      if (!next[value.id] && Object.keys(next).length >= 100) {
        const cold = Object.keys(next).find(id => !next[id]!.dirty)
        if (cold) delete next[cold]
        else throw new Error('同时编辑的容器已达到 100 项，请先保存部分配置后再打开')
      }
      containers.value = { ...next, [value.id]: { value, expectedAuth: value.auth, dirty: false } }
    }
    return containers.value[value.id]!
  }
  function edit(value: WorkspaceContainer) {
    const current = containers.value[value.id]
    if (!current) return
    containers.value = { ...containers.value, [value.id]: { ...current, value, dirty: true } }
  }
  function baseline(id: string, auth: AuthConfig) {
    const current = containers.value[id]
    if (current) containers.value = { ...containers.value, [id]: { ...current, expectedAuth: auth } }
  }
  const containerSaves = new Map<string, Promise<boolean>>()
  async function saveContainer(id: string): Promise<boolean> {
    const existing = containerSaves.get(id)
    if (existing) return existing
    const version = generation
    const task = (async () => {
      try {
        while (version === generation) {
          const entry = containers.value[id]
          if (!entry?.dirty) return true
          const saved = await ipc.workspaceSaveContainer(entry.value, entry.expectedAuth)
          if (version !== generation) return false
          const current = containers.value[id]
          if (!current) return false
          // Token rotation may advance the optimistic baseline while a save is pending.
          const expectedAuth = current.expectedAuth === entry.expectedAuth ? saved.auth : current.expectedAuth
          containers.value = { ...containers.value, [id]: { ...current, expectedAuth,
            ...(current.value === entry.value ? { value: saved, dirty: false } : {}) } }
        }
      } catch (error) {
        if (version === generation) {
          const normalized = normalizeError(error)
          lastError.value = normalized.message === 'AUTH_CONFIG_STALE'
            ? { kind: 'io', message: '认证来源已更新，未覆盖新凭据', hints: [{ label: '保留当前编辑，重新加载配置后手动合并' }] } : normalized
        }
      }
      return false
    })()
    containerSaves.set(id, task)
    void task.then(() => { if (containerSaves.get(id) === task) containerSaves.delete(id) })
    return task
  }
  async function closeDraft(tab: Tab): Promise<boolean> {
    lastError.value = null
    const version = generation
    const { confirmUnsaved } = await import('@/features/feedback')
    const choice = await confirmUnsaved(`“${tab.title}”尚未保存到集合。保存草稿后，下次打开此工作区会恢复。`, '保存草稿')
    if (choice === 'cancel' || version !== generation) return false
    const current = useTabsStore().tabs.find(item => item.id === tab.id)
    if (current !== tab) return false
    // Keep the tab in the snapshot until removal; archive only after the caller removes it.
    if (choice === 'discard') { excludedDrafts.add(tab.recoveryId); revision++ }
    const written = await flush()
    excludedDrafts.delete(tab.recoveryId)
    if (!written || useTabsStore().tabs.find(item => item.id === tab.id) !== tab) { schedule(); return false }
    if (choice === 'save') archived = [...archived, draft(tab)]
    return true
  }
  async function settleContainers(ids = Object.keys(containers.value)): Promise<boolean> {
    lastError.value = null
    const dirty = ids.filter(id => containers.value[id]?.dirty)
    if (!dirty.length) return true
    const version = generation
    const { confirmUnsaved } = await import('@/features/feedback')
    const choice = await confirmUnsaved(`有 ${dirty.length} 项容器配置尚未保存。`)
    if (choice === 'cancel' || version !== generation) return false
    const before = containers.value
    for (const id of dirty) {
      if (choice === 'save') { if (!(await saveContainer(id))) return false }
      else { const next = { ...containers.value }; delete next[id]; containers.value = next }
    }
    if (await flush()) return true
    if (version === generation && choice === 'discard') containers.value = before
    return false
  }
  async function prepareLeave(): Promise<boolean> {
    lastError.value = null
    const version = generation
    const tabs = useTabsStore()
    const drafts = tabs.tabs.filter(tab => tab.kind === 'request' && !tab.nodeId && tab.dirty && !tab.historyPreview)
    const dirtyContainers = Object.values(containers.value).filter(entry => entry.dirty)
    const before = containers.value
    let discarded = false
    if (drafts.length || dirtyContainers.length) {
      const { confirmUnsaved } = await import('@/features/feedback')
      const choice = await confirmUnsaved(`有 ${drafts.length} 个请求草稿和 ${dirtyContainers.length} 项容器配置尚未保存。请求草稿保存在本机，下次打开此工作区会恢复。`, '保存配置与草稿')
      if (choice === 'cancel' || version !== generation) return false
      if (drafts.some(tab => tabs.tabs.find(item => item.id === tab.id) !== tab)) return false
      if (choice === 'save') {
        for (const item of dirtyContainers) if (!(await saveContainer(item.value.id))) return false
      } else {
        discarded = true
        for (const tab of drafts) excludedDrafts.add(tab.recoveryId)
        revision++
        for (const item of dirtyContainers) forgetContainer(item.value.id)
      }
    }
    const written = await flush()
    if (version !== generation) return false
    if (discarded) {
      for (const tab of drafts) excludedDrafts.delete(tab.recoveryId)
      if (written) for (const tab of drafts) tabs.patch(tab.id, { dirty: false })
      else containers.value = before
    }
    return written
  }
  function archiveBodyDrafts(tab: Tab) {
    archived = [...archived.filter(item => item.nodeId !== tab.nodeId), ...(Object.keys(tab.bodyDrafts).length ? [draft(tab)] : [])]
    schedule()
  }
  function assertBodyDraftCapacity(tab: Tab, bodyDrafts: BodyDrafts, body: Body) {
    const saved = snapshot()
    const others = saved.requests.filter(item => item.id !== tab.recoveryId)
    if (others.length + saved.containers.length >= 100) throw new Error('恢复草稿达到 100 项上限，请先保存或关闭其他编辑')
    if (others.reduce((bytes, item) => bytes + bodyDraftBytes(item.bodyDrafts ?? {}), bodyDraftBytes(bodyDrafts)) > BODY_DRAFT_BYTES) {
      throw new Error('Body 类型草稿达到 8 MiB 上限，请先清除不用的类型草稿')
    }
    const candidate = { requests: [...others, draft({ ...tab, body, bodyDrafts, dirty: true })], containers: saved.containers }
    if (new TextEncoder().encode(JSON.stringify(candidate)).byteLength > BODY_DRAFT_BYTES) {
      throw new Error('本机恢复草稿达到 8 MiB 上限，请先保存或清除不用的编辑')
    }
  }
  function takeBodyDrafts(nodeId: string) {
    const item = archived.find(item => item.nodeId === nodeId)
    if (!item) return undefined
    archived = archived.filter(entry => entry !== item)
    return item.bodyDrafts
  }
  function forgetContainer(id: string) { const next = { ...containers.value }; delete next[id]; containers.value = next }
  return { assertBodyDraftCapacity, archiveBodyDrafts, takeBodyDrafts, containers, lastError, start, stop, flush, put, edit, baseline, saveContainer, closeDraft, settleContainers, prepareLeave, forgetContainer }
})
