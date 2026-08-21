/**
 * IPC Facade —— 前端与 Rust 通信的**唯一**出口（03 文档 §2.1）。
 *
 * 组件不允许直接 `import { invoke } from '@tauri-apps/api/core'`。
 * 全部走这一层，理由：
 *   1. 错误归一化：Rust 侧返回统一的 AppError 结构，这里转成前端可渲染的形状
 *   2. 大数据不走 invoke：响应体一律走 `apogee://` 自定义协议 + Range
 *   3. 可测试：Spike 和单测里可以整体替换这一层
 */

import { invoke as tauriInvoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'

// ─────────────────────────────────────────────────────────────────────
// 类型契约
//
// 正式实现里这些类型由 ts-rs 从 Rust 侧生成到 src/types/generated/
// （03 文档 §9「类型契约」），避免手工同步产生偏差。
// 骨架阶段先手写，字段名与 crates/apogee-core/src 保持一致。
// ─────────────────────────────────────────────────────────────────────

export type ErrorKind =
  | 'dns' | 'connect' | 'tls' | 'timeout'
  | 'http' | 'io' | 'parse' | 'script' | 'var'

export interface PublicHint {
  label: string
  action?: string
}

/** Alias retained for components that only render public error data. */
export type Hint = PublicHint

/**
 * Error payload allowed across request/history IPC. Technical detail is intentionally absent:
 * it belongs to the internal execution diagnostic path, not ordinary renderer state.
 */
export interface PublicError {
  code?: string
  kind: ErrorKind
  message: string
  /** 可操作建议。空数组是设计缺陷，见 02 文档 §8 的三段式错误要求。 */
  hints: PublicHint[]
}

export type AppError = PublicError

/** 响应体渲染分级。**唯一的高亮阈值是 2MB**（02 文档 §4.4）。 */
export type RenderTier = 'full' | 'virtualized' | 'noHighlight' | 'summaryOnly'

export interface Timing {
  prepareMs: number
  networkPhasesAvailable: boolean
  dnsMs: number
  tcpMs: number
  tlsMs: number
  requestMs: number
  ttfbMs: number
  downloadMs: number
  totalMs: number
}

/**
 * Sanitized execution snapshot. Request bodies, body parameters, file paths and credentials are
 * intentionally not present in the renderer-facing contract.
 */
export interface PublicActualRequest {
  method: string
  url: string
  /** 已完成变量解析但已经脱敏的路径参数值。 */
  pathParams?: [string, string][]
  headers: [string, string][]
  /** 已知浏览器/协议通用 Header 名称，供实际请求表格默认隐藏。 */
  commonHeaderNames?: string[]
}

/** Backwards-compatible UI name for the public-only request snapshot. */
export type ActualRequest = PublicActualRequest

/**
 * Sanitized response metadata. 响应体仍只通过受 session 保护的 apogee:// Range 通道读取。
 */
export interface PublicResponseSummary {
  id: string
  sizeBytes: number
  tier: RenderTier
  syntaxHighlight: boolean
  contentType?: string
  indexed: boolean
  totalNodes?: number
  /** SpanIndex 的近似常驻堆内存，不包含响应体文件或 mmap。 */
  indexBytes?: number
  timing: Timing
  actualRequest?: PublicActualRequest
  // ── HTTP 语义（03 文档 §3.2）──
  // 全部可选：Spike 合成响应没有 HTTP 语义，UI 据此显示「不可用」而非编造 200。
  status?: number
  statusText?: string
  httpVersion?: string
  headers?: [string, string][]
  charset?: string
  truncated?: boolean
}

/** Existing UI consumers use this alias; it can never be a raw internal Rust summary. */
export type ResponseSummary = PublicResponseSummary

export interface MemInfo {
  workingSetBytes: number
  privateBytes: number
  /** false 表示该平台还没实现读取，UI 应显示「不可用」而不是「0 MB」 */
  available: boolean
}

export type AppMemoryPartKind = 'main' | 'browser' | 'renderer' | 'gpu' | 'utility' | 'other'

export interface AppMemoryPart {
  kind: AppMemoryPartKind
  processCount: number
  workingSetBytes: number
  privateBytes: number
}

/** Apogee 进程树内存：含主进程及 WebView 子进程。 */
export interface AppMemInfo extends MemInfo {
  processCount: number
  parts: AppMemoryPart[]
}

/** 前端阶段耗时；S1 同时记录这些应用内时钟与外部进程启动时钟。 */
export interface BootMetrics {
  mountMs: number
  readyMs: number
  firstPaintMs: number
}

export type WorkspaceNodeKind = 'collection' | 'folder' | 'request'

export interface WorkspaceNode {
  id: string
  parentId?: string | null
  depth: number
  name: string
  method?: string
  isFolder: boolean
  expanded: boolean
  kind?: WorkspaceNodeKind
  /** 请求下保存的调试用例数量；完整定义在用户展开时按需读取。 */
  debugCaseCount: number
}

export interface EnvironmentVariable {
  key: string
  value: string
  enabled: boolean
}

export interface Environment {
  id: string
  name: string
  host: string
  variables: EnvironmentVariable[]
}

export interface WorkspaceSnapshot {
  id: string
  name: string
  nodes: WorkspaceNode[]
  environments: Environment[]
}

export interface WorkspaceState {
  activeEnvironmentId: string | null
  activeEnvironmentSet: boolean
}

export type WorkspaceStatus = 'available' | 'missing' | 'invalid'

export interface WorkspaceDescriptor {
  id: string
  name: string
  path: string
  lastOpenedAt: number
  status: WorkspaceStatus
}

export interface AppSettings {
  settingsVersion: number
  defaultWorkspaceRoot: string
  openLastWorkspaceOnStart: boolean
  showMemoryMonitor: boolean
  lowMemoryMode: boolean
  proxyMode: 'system' | 'manual' | 'none'
  proxyUrl?: string
  timeoutMs: number
  verifyTls: boolean
  themeMode: 'dark' | 'light' | 'system'
  language: 'system' | 'zh-CN' | 'en-US'
  fontFamily: string
  /** 请求 URL、参数、请求头和请求体编辑器使用的字体。 */
  editorFontFamily: string
  uiFontSize: number
  resultFontSize: number
}

export interface WorkspaceOpenResult {
  descriptor: WorkspaceDescriptor
  workspaceId: string
  sessionId: string
  snapshot: WorkspaceSnapshot
  state: WorkspaceState
}

export interface WorkspaceRequest {
  id: string
  name: string
  spec: RequestSpec
}

export type DebugCaseType = 'success' | 'failure' | 'exception'

/** 归属已保存请求、可复用参数的命名调试用例。 */
export interface DebugCase {
  id: string
  requestId: string
  name: string
  /** 保存时选择的预期场景；运行结果仍独立由 HTTP 状态或网络错误决定。 */
  caseType: DebugCaseType
  /** 保存时的未解析请求快照，保留 {{variable}} 占位符。 */
  spec: RequestSpec
  createdAtMs: number
  updatedAtMs: number
}

export interface HistoryEntry {
  id: string
  createdAtMs: number
  /** 未关联调试用例的普通请求历史为 null。 */
  debugCaseId?: string | null
  /** 执行时选中的环境；用于按环境显示用例最近结果。 */
  environmentId?: string | null
  method: string
  url: string
  /** 未收到 HTTP 响应的运行没有状态码，界面显示为 ERR。 */
  status?: number
  sizeBytes: number
  durationMs: number
}

/** A non-executable, redacted history request view. It never carries body or auth configuration. */
export interface HistoryRequestPreview {
  method: string
  url: string
  params?: PublicKeyValue[]
  pathParams?: PublicKeyValue[]
  headers?: PublicKeyValue[]
  settings?: PublicRequestSettings
}

/** A public parameter/header row. Free-form descriptions are not retained in history IPC. */
export interface PublicKeyValue {
  enabled: boolean
  key: string
  value: string
  kind?: 'text' | 'file' | 'secret'
}

/** 成功运行带 response；失败运行带 error、可选的安全实际请求，且没有响应体。 */
export interface HistoryRecord {
  id: string
  createdAtMs: number
  debugCaseId?: string | null
  environmentId?: string | null
  spec: HistoryRequestPreview
  response?: PublicResponseSummary | null
  error?: PublicError | null
  actualRequest?: PublicActualRequest | null
  durationMs: number
}

export type NodeKind = 'object' | 'array' | 'string' | 'number' | 'bool' | 'null'

/** 扁平树节点。**没有 children 字段** —— 前端不做递归构造（03 文档 §4.4）。 */
export interface TreeNode {
  path: string
  key: string
  kind: NodeKind
  preview?: string
  childCount: number
  depth: number
}

// ─────────────────────────────────────────────────────────────────────
// 底座
// ─────────────────────────────────────────────────────────────────────

/**
 * 是否跑在具备完整 IPC 能力的 Tauri 宿主里。
 *
 * 仅检查 `__TAURI_INTERNALS__` 这个属性不够：某些 WebView/浏览器调试环境
 * 会暴露一个空的内部对象，随后调用 `invoke` 只会得到难以定位的
 * `Cannot read properties of undefined (reading 'invoke')`。
 */
export const inTauri =
  typeof window !== 'undefined' &&
  typeof (window as Window & { __TAURI_INTERNALS__?: { invoke?: unknown } })
    .__TAURI_INTERNALS__?.invoke === 'function'

function isAppError(e: unknown): e is AppError {
  return typeof e === 'object' && e !== null && 'kind' in e && 'message' in e
}

/** 把任意抛出物归一化成 AppError，保证 UI 永远拿得到 hints 数组。 */
export function normalizeError(e: unknown): AppError {
  if (isAppError(e)) return e
  if (typeof e === 'string') {
    const match = e.match(/^([A-Z][A-Z0-9_]{2,}):?\s*(.*)$/)
    if (match) return { code: match[1], kind: 'io', message: match[2] || match[1], hints: [] }
  }
  return {
    kind: 'io',
    message: e instanceof Error ? e.message : String(e),
    hints: [],
  }
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri) {
    throw {
      kind: 'io',
      message: '当前页面未连接到 Apogee 桌面宿主，无法执行请求',
      hints: [{ label: '请从 Apogee 桌面应用启动，而不是直接打开 Vite 预览页' }],
    } satisfies AppError
  }
  try {
    return await tauriInvoke<T>(cmd, args)
  } catch (e) {
    throw normalizeError(e)
  }
}

export interface WorkspaceSession {
  workspaceId: string
  sessionId: string
}

let workspaceSession: WorkspaceSession | null = null

function sessionArgs(): WorkspaceSession {
  if (!workspaceSession) throw new Error('当前没有打开的工作区')
  return workspaceSession
}

export function setWorkspaceSession(session: WorkspaceSession | null) {
  // `workspaceOpen` 返回的对象还带 descriptor/snapshot；只保留授权边界需要的两项，
  // 避免以后把整个打开结果意外透传给每个 command。
  workspaceSession = session && {
    workspaceId: session.workspaceId,
    sessionId: session.sessionId,
  }
}

// ─────────────────────────────────────────────────────────────────────
// 命令（03 文档 §3.2）
// ─────────────────────────────────────────────────────────────────────

export interface YapiCategorySummary {
  name: string
  requestCount: number
}

export interface YapiAnalyzeResult {
  planId: string
  projectName: string
  categories: YapiCategorySummary[]
  requestCount: number
  environmentCount: number
  warnings: string[]
}

export interface YapiApplyOptions {
  collectionName?: string
  importEnvironments: boolean
}

export interface YapiImportResult {
  collectionId: string
  collectionName: string
  importedRequests: number
  importedFolders: number
  importedEnvironments: number
  warnings: string[]
}

export const ipc = {
  appSettingsLoad(): Promise<AppSettings> {
    return call('app_settings_load')
  },

  appSettingsSave(settings: AppSettings): Promise<AppSettings> {
    return call('app_settings_save', { settings })
  },

  systemFontFamilies(): Promise<string[]> {
    return call('system_font_families')
  },

  appRestart(): Promise<void> {
    return call('app_restart')
  },
  /** 生成一个用于压测的合成响应体（Spike S2 用，不是产品功能）。 */
  spikeMakeResponse(targetBytes: number): Promise<ResponseSummary> {
    return call('spike_make_response', { targetBytes })
  },

  /** 取响应摘要。**不含 body。** */
  getResponseMeta(responseId: string): Promise<ResponseSummary> {
    return call('get_response_meta', { ...sessionArgs(), responseId })
  },

  /** 按路径取一页树节点。path 为空字符串表示根。 */
  getResponseTree(
    responseId: string,
    path: string,
    offset: number,
    limit: number,
  ): Promise<TreeNode[]> {
    return call('get_response_tree', { ...sessionArgs(), responseId, path, offset, limit })
  },

  /** 让 Rust 侧解析 JSON 并建索引。大 body 下这一步是显式的、可跳过的。 */
  indexResponse(responseId: string): Promise<ResponseSummary> {
    return call('index_response', { ...sessionArgs(), responseId })
  },

  /** Spike 只访问 Rust 标记为 Synthetic 的响应，不携带普通 workspace response scope。 */
  spikeIndexResponse(responseId: string): Promise<ResponseSummary> {
    return call('spike_index_response', { responseId })
  },

  spikeGetResponseTree(
    responseId: string,
    path: string,
    offset: number,
    limit: number,
  ): Promise<TreeNode[]> {
    return call('spike_get_response_tree', { responseId, path, offset, limit })
  },

  /** 当前进程内存。供「性能自检」面板与性能基准使用（PRD §5.2）。 */
  processMemory(): Promise<MemInfo> {
    return call('process_memory')
  },

  /** 当前 Apogee 进程树的内存总量及分类明细。 */
  appMemory(): Promise<AppMemInfo> {
    return call('app_memory')
  },

  /** 把 Spike 的量测结果交给 Rust 侧落盘（前端拿不到文件系统权限）。 */
  spikeReport(name: string, json: string): Promise<string> {
    return call('spike_report', { name, json })
  },

  /**
   * 本次启动要不要自动跑某个 Spike（由 `APOGEE_SPIKE_AUTORUN` 环境变量给）。
   *
   * 运行时开关而非构建期开关：S3 要在**同一个 release 二进制**上跑「GPU 开」
   * 与「GPU 关」两种配置，构建期开关会导致每换一次配置就重链一次，
   * 也就无法保证两轮测的是同一份代码。
   */
  spikeAutorun(): Promise<string | null> {
    return call('spike_autorun')
  },

  /**
   * 报告首帧就绪 → Rust 侧显示窗口（03 文档 §4.1 分阶段启动）。
   * 窗口以 visible:false 创建，避免白屏闪烁。
   */
  appReady(metrics: BootMetrics): Promise<void> {
    return call('app_ready', { metrics })
  },

  // ── 工作区持久化（03 文档 §3.2 / §6）─────────────────────────────

  workspaceList(): Promise<WorkspaceDescriptor[]> {
    return call('workspace_list')
  },

  async workspaceCreate(path: string | undefined, name: string): Promise<WorkspaceOpenResult> {
    const result = await call<WorkspaceOpenResult>('workspace_create', { path, name })
    setWorkspaceSession(result)
    return result
  },

  async workspaceOpen(input: { workspaceId?: string; path?: string }): Promise<WorkspaceOpenResult> {
    const result = await call<WorkspaceOpenResult>('workspace_open', input)
    setWorkspaceSession(result)
    return result
  },

  async workspaceClose(): Promise<void> {
    const session = sessionArgs()
    await call('workspace_close', { ...session })
    setWorkspaceSession(null)
  },

  workspaceForget(workspaceId: string): Promise<void> {
    return call('workspace_forget', { workspaceId })
  },

  workspaceRelocate(workspaceId: string, path: string): Promise<WorkspaceDescriptor> {
    return call('workspace_relocate', { workspaceId, path })
  },

  workspaceReveal(workspaceId: string): Promise<void> {
    return call('workspace_reveal', { workspaceId })
  },

  /** 读取 Rust 侧已经扁平化的集合树。 */
  workspaceLoad(): Promise<WorkspaceSnapshot> {
    return call('workspace_load', { ...sessionArgs() })
  },

  workspaceCreateEnvironment(name: string): Promise<Environment> {
    return call('workspace_create_environment', { ...sessionArgs(), name })
  },

  workspaceSaveEnvironment(environment: Environment): Promise<Environment> {
    return call('workspace_save_environment', { ...sessionArgs(), environment })
  },

  workspaceDeleteEnvironment(environmentId: string): Promise<WorkspaceSnapshot> {
    return call('workspace_delete_environment', { ...sessionArgs(), environmentId })
  },

  workspaceSaveState(activeEnvironmentId: string | null): Promise<void> {
    return call('workspace_save_state', {
      ...sessionArgs(),
      workspaceState: { activeEnvironmentId, activeEnvironmentSet: true },
    })
  },

  /** 读取一个请求的完整编辑模型。 */
  workspaceReadRequest(requestId: string): Promise<WorkspaceRequest> {
    return call('workspace_read_request', { ...sessionArgs(), requestId })
  },

  /** 保存请求的持久化字段；请求体和大响应体都不通过额外的前端缓存传递。 */
  workspaceSaveRequest(requestId: string, name: string, spec: RequestSpec): Promise<void> {
    return call('workspace_save_request', { ...sessionArgs(), requestId, name, spec })
  },

  /** 读取绑定一个请求的命名调试用例定义。 */
  workspaceListDebugCases(requestId: string): Promise<DebugCase[]> {
    return call('workspace_list_debug_cases', { ...sessionArgs(), requestId })
  },

  /** 将当前未解析请求参数以预期场景分类保存为调试用例。 */
  workspaceCreateDebugCase(
    requestId: string,
    name: string,
    caseType: DebugCaseType,
    spec: RequestSpec,
  ): Promise<DebugCase> {
    return call('workspace_create_debug_case', { ...sessionArgs(), requestId, name, caseType, spec })
  },

  workspaceDeleteDebugCase(requestId: string, caseId: string): Promise<void> {
    return call('workspace_delete_debug_case', { ...sessionArgs(), requestId, caseId })
  },

  workspaceCreateCollection(name: string): Promise<WorkspaceSnapshot> {
    return call('workspace_create_collection', { ...sessionArgs(), name })
  },

  workspaceCreateFolder(parentId: string, name: string): Promise<WorkspaceSnapshot> {
    return call('workspace_create_folder', { ...sessionArgs(), parentId, name })
  },

  workspaceCreateRequest(parentId: string, name: string): Promise<WorkspaceRequest> {
    return call('workspace_create_request', { ...sessionArgs(), parentId, name })
  },

  workspaceRenameNode(nodeId: string, name: string): Promise<WorkspaceSnapshot> {
    return call('workspace_rename_node', { ...sessionArgs(), nodeId, name })
  },

  workspaceDeleteNode(nodeId: string): Promise<WorkspaceSnapshot> {
    return call('workspace_delete_node', { ...sessionArgs(), nodeId })
  },

  // ── 请求执行（03 文档 §3.2 / §4.5）─────────────────────────────────

  /** 发送一个 HTTP 请求。立即返回 ExecHandle，不等网络。 */
  sendRequest(
    spec: RequestSpec,
    ctx?: ExecContext,
    session: WorkspaceSession = sessionArgs(),
  ): Promise<ExecHandle> {
    return call('send_request', { ...session, spec, ctx })
  },

  /** 取消一个正在执行的请求。返回 false 表示请求已结束或不存在。 */
  cancelRequest(handleId: string): Promise<boolean> {
    return call('cancel_request', { handleId })
  },

  historyRecord(
    createdAtMs: number,
    spec: HistoryRequestPreview,
    response: PublicResponseSummary,
    debugCaseId?: string,
    environmentId?: string,
    session: WorkspaceSession = sessionArgs(),
  ): Promise<void> {
    return call('history_record', {
      input: {
        ...session,
        createdAtMs,
        spec,
        response,
        debugCaseId,
        environmentId,
      },
    })
  },

  /** 持久化未收到 HTTP 响应的失败运行；不传也不创建响应体。 */
  historyRecordError(
    createdAtMs: number,
    spec: HistoryRequestPreview,
    error: PublicError,
    actualRequest: PublicActualRequest | null,
    durationMs: number,
    debugCaseId?: string,
    environmentId?: string,
    session: WorkspaceSession = sessionArgs(),
  ): Promise<void> {
    return call('history_record_error', {
      input: {
        ...session,
        createdAtMs,
        spec,
        error,
        actualRequest,
        durationMs,
        debugCaseId,
        environmentId,
      },
    })
  },

  historyList(): Promise<HistoryEntry[]> {
    return call('history_list', { ...sessionArgs() })
  },

  historyRead(historyId: string): Promise<HistoryRecord> {
    return call('history_read', { ...sessionArgs(), historyId })
  },

  historyClear(): Promise<void> {
    return call('history_clear', { ...sessionArgs() })
  },

  curlImport(command: string): Promise<{ spec: RequestSpec; warnings: string[] }> {
    return call('curl_import', { command })
  },

  workspaceReadContainer(nodeId: string): Promise<WorkspaceContainer> {
    return call('workspace_read_container', { ...sessionArgs(), nodeId })
  },

  workspaceSaveContainer(container: WorkspaceContainer): Promise<WorkspaceContainer> {
    return call('workspace_save_container', { ...sessionArgs(), container })
  },

  postmanImport(json: string): Promise<{ name: string; requests: Array<{ name: string; folder: string[]; spec: RequestSpec }>; warnings: string[] }> {
    return call('postman_import', { json })
  },
  postmanImportFile(path: string): Promise<{ name: string; requests: Array<{ name: string; folder: string[]; spec: RequestSpec }>; warnings: string[] }> {
    return call('postman_import_file', { path })
  },
  postmanImportWorkspace(path: string, importId: string): Promise<{ collectionId: string; importedRequests: number; importedFolders: number; warnings: string[] }> {
    return call('postman_import_workspace', { ...sessionArgs(), path, importId })
  },
  postmanImportEnvironmentWorkspace(path: string, importId: string): Promise<{ environment: Environment; warnings: string[] }> {
    return call('postman_import_environment_workspace', { ...sessionArgs(), path, importId })
  },
  yapiAnalyzeImport(baseUrl: string, token: string): Promise<YapiAnalyzeResult> {
    return call('yapi_analyze_import', { baseUrl, token })
  },
  yapiApplyImport(planId: string, options: YapiApplyOptions): Promise<YapiImportResult> {
    return call('yapi_apply_import', { ...sessionArgs(), planId, options })
  },
  yapiDiscardImport(planId: string): Promise<void> {
    return call('yapi_discard_import', { planId })
  },
}

// ─────────────────────────────────────────────────────────────────────
// 请求事件（03 文档 §3.2 / §4.5）
// ─────────────────────────────────────────────────────────────────────

import { listen as tauriListen } from '@tauri-apps/api/event'

export interface RequestEventScope {
  workspaceId: string
  sessionId: string
}

export interface ProgressEvent extends RequestEventScope {
  id: string
  received: number
  total: number | null
}

export interface DoneEvent extends RequestEventScope {
  response: ResponseSummary
}

export interface ErrorEvent extends RequestEventScope {
  id: string
  error: AppError
  /** 请求已组装后发生网络错误时携带，用于仍然展示实际请求。 */
  actualRequest?: ActualRequest | null
}

export interface CancelledEvent extends RequestEventScope {
  id: string
}

export interface PostmanImportProgressEvent {
  importId: string
  current: number
  total: number
  phase: string
}

/** Subscribe before invoking an import so its earliest progress events are not lost. */
export async function onPostmanImportProgress(
  importId: string,
  handler: (event: PostmanImportProgressEvent) => void,
): Promise<() => void> {
  if (!inTauri) return () => {}
  return tauriListen<PostmanImportProgressEvent>('postman-import:progress', (event) => {
    if (event.payload.importId === importId) handler(event.payload)
  })
}

/** 请求事件的统一分发。
 *
 * 调用方按事件类型注册回调。返回 clean-up 函数，组件销毁时调用即可，
 * 内部自动取消所有四个监听。
 */
export function onRequestEvents(handlers: {
  onProgress?: (e: ProgressEvent) => void
  onDone?: (e: DoneEvent) => void
  onError?: (e: ErrorEvent) => void
  onCancelled?: (e: CancelledEvent) => void
}): () => void {
  if (!inTauri) return () => {}

  const unlistens: (() => void)[] = []
  // `listen` 是异步的。如果清理函数在这四个 Promise resolve 之前就被调用
  // （组件立刻卸载、HMR、Spike 面板反复挂载），unlistens 还是空数组 ——
  // 那些监听就再也没人取消了，事件回调会打到已销毁的组件上。
  // 所以要记住「已经清理过」，让后到的 UnlistenFn 自己就地退订。
  let disposed = false

  const subs: [string, (payload: unknown) => void][] = [
    ['request:progress', (p) => handlers.onProgress?.(p as ProgressEvent)],
    ['request:done', (p) => handlers.onDone?.(p as DoneEvent)],
    ['request:error', (p) => handlers.onError?.(p as ErrorEvent)],
    ['request:cancelled', (p) => handlers.onCancelled?.(p as CancelledEvent)],
  ]

  for (const [event, cb] of subs) {
    tauriListen<unknown>(event, (e) => cb(e.payload)).then((ul) => {
      if (disposed) ul()
      else unlistens.push(ul)
    })
  }

  return () => {
    disposed = true
    while (unlistens.length) unlistens.pop()?.()
  }
}

// ─────────────────────────────────────────────────────────────────────
// 请求类型（同步自 crates/apogee-core/src/request.rs）
// ─────────────────────────────────────────────────────────────────────

export interface RequestSpec {
  method: string
  url: string
  params?: KeyValue[]
  pathParams?: KeyValue[]
  headers?: KeyValue[]
  body?: Body
  auth?: AuthConfig
  settings?: RequestSettings
}

export interface KeyValue {
  enabled: boolean
  key: string
  value: string
  description?: string
  kind?: 'text' | 'file' | 'secret'
}

/** 请求体。`tag = "type"` 的 JSON 可辨识联合。 */
export type Body =
  | { type: 'none' }
  | { type: 'json'; content: string }
  | { type: 'text'; content: string; contentType: string }
  | { type: 'formData'; rows: KeyValue[] }
  | { type: 'urlEncoded'; rows: KeyValue[] }
  | { type: 'binary'; path: string }
  | { type: 'graphQl'; query: string; variables: string; operationName?: string }

export type AuthConfig =
  | { type: 'inherit' }
  | { type: 'none' }
  | { type: 'basic'; username: string; password: string }
  | { type: 'bearer'; token: string }
  | { type: 'apiKey'; key: string; value: string; location: 'header' | 'query' | 'cookie' }

/** 可公开显示的请求设置；连接代理地址不属于该 DTO。 */
export interface PublicRequestSettings {
  timeoutMs?: number
  followRedirects?: boolean
  maxRedirects?: number
  verifyTls?: boolean
  httpVersion?: 'http1' | 'http2'
  encodeUrl?: boolean
  sendCookies?: boolean
  retryCount?: number
  retryBackoffMs?: number
}

/** 执行输入当前不暴露 proxy URL，和公开设置形状相同。 */
export type RequestSettings = PublicRequestSettings

export interface ExecContext {
  tabId?: string
  environmentId?: string
  requestNodeId?: string
}

export interface WorkspaceContainer {
  id: string
  name: string
  auth: AuthConfig
  settings: RequestSettings
}

export interface ExecHandle {
  id: string
}

// ─────────────────────────────────────────────────────────────────────
// History/public DTO conversion
//
// The renderer still owns the editable RequestSpec. It must never forward that object to
// history IPC: this conversion deliberately constructs a new non-executable preview and omits
// body/auth/proxy fields even if an untyped caller adds them to the input object.
// ─────────────────────────────────────────────────────────────────────

const PUBLIC_REDACTED = '[redacted]'
const PUBLIC_FILE_OMITTED = '[file omitted]'

function normalizedSensitiveName(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]/g, '')
}

function isSensitivePublicName(name: string): boolean {
  const normalized = normalizedSensitiveName(name)
  if (!normalized) return false
  const exact = new Set([
    'authorization', 'proxyauthorization', 'cookie', 'setcookie', 'apikey', 'xapikey',
    'accesskey', 'secret', 'clientsecret', 'token', 'accesstoken', 'refreshtoken',
    'idtoken', 'password', 'passwd', 'pwd', 'signature', 'sig', 'credential',
    'credentials', 'bearer', 'privatekey', 'session', 'sessionid', 'key',
  ])
  return exact.has(normalized) || [
    'apikey', 'token', 'secret', 'password', 'signature', 'credential', 'authorization', 'cookie',
  ].some((marker) => normalized.includes(marker))
}

function isSensitivePublicValue(value: string): boolean {
  const normalized = value.trimStart().toLowerCase()
  return normalized.startsWith('bearer ')
    || normalized.startsWith('basic ')
    || normalized.startsWith('token ')
    || normalized.startsWith('apikey ')
    || normalized.startsWith('api-key ')
}

function hasSensitiveKeyValue(row: KeyValue): boolean {
  return row.kind === 'file'
    || row.kind === 'secret'
    || isSensitivePublicName(row.key)
    || isSensitivePublicValue(row.value)
}

function toPublicKeyValue(row: KeyValue, sensitiveNames: string[] = []): PublicKeyValue {
  const sourceMarkedSensitive = isMarkedSensitiveName(row.key, sensitiveNames)
  const value = row.kind === 'file'
    ? PUBLIC_FILE_OMITTED
    : hasSensitiveKeyValue(row) || sourceMarkedSensitive
      ? PUBLIC_REDACTED
      : row.value
  return {
    enabled: row.enabled,
    key: row.key,
    value,
    kind: row.kind,
  }
}

/** Matches Rust's `urlencoding_path`: preserve slash, percent-encode every other byte. */
function encodePathValue(value: string): string {
  return Array.from(new TextEncoder().encode(value), (byte) => {
    const isUnreserved = (byte >= 0x41 && byte <= 0x5a)
      || (byte >= 0x61 && byte <= 0x7a)
      || (byte >= 0x30 && byte <= 0x39)
      || byte === 0x2d || byte === 0x5f || byte === 0x2e || byte === 0x7e || byte === 0x2f
    return isUnreserved ? String.fromCharCode(byte) : `%${byte.toString(16).padStart(2, '0').toUpperCase()}`
  }).join('')
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** Match percent escapes case-insensitively without making unescaped path bytes case-insensitive. */
function encodedPathPattern(encoded: string): RegExp {
  let pattern = ''
  for (let index = 0; index < encoded.length;) {
    if (encoded[index] === '%' && index + 2 < encoded.length) {
      const first = encoded[index + 1] ?? ''
      const second = encoded[index + 2] ?? ''
      pattern += `%[${first.toUpperCase()}${first.toLowerCase()}][${second.toUpperCase()}${second.toLowerCase()}]`
      index += 3
    } else {
      pattern += escapeRegExp(encoded[index] ?? '')
      index += 1
    }
  }
  return new RegExp(pattern, 'g')
}

function redactLiteralValues(value: string, sensitiveValues: string[]): string {
  const ordered = [...new Set(sensitiveValues.filter((sensitive) => (
    sensitive.length > 0 && sensitive !== PUBLIC_REDACTED && sensitive !== PUBLIC_FILE_OMITTED
  )))].sort((left, right) => right.length - left.length)

  return ordered.reduce((result, sensitive) => {
    const rawRedacted = result.split(sensitive).join(PUBLIC_REDACTED)
    const encoded = encodePathValue(sensitive)
    return encoded === sensitive
      ? rawRedacted
      : rawRedacted.replace(encodedPathPattern(encoded), PUBLIC_REDACTED)
  }, value)
}

function decodeQueryName(value: string): string {
  try {
    return decodeURIComponent(value.replaceAll('+', ' '))
  } catch {
    return value
  }
}

function isMarkedSensitiveName(name: string, sensitiveNames: string[]): boolean {
  const normalized = name.trim().toLowerCase()
  return normalized.length > 0 && sensitiveNames.some(
    (sensitiveName) => sensitiveName.trim().toLowerCase() === normalized,
  )
}

function stripUrlUserinfo(value: string): string {
  const schemeEnd = value.indexOf('://')
  if (schemeEnd < 0) return value
  const authorityStart = schemeEnd + 3
  const rest = value.slice(authorityStart)
  const slash = rest.indexOf('/')
  const authority = slash < 0 ? rest : rest.slice(0, slash)
  const suffix = slash < 0 ? '' : rest.slice(slash)
  const at = authority.lastIndexOf('@')
  return at < 0 ? value : `${value.slice(0, authorityStart)}${authority.slice(at + 1)}${suffix}`
}

function redactUnparseableUrl(value: string, sensitiveQueryNames: string[]): string {
  const withoutFragment = value.split('#', 1)[0] ?? ''
  const queryStart = withoutFragment.indexOf('?')
  if (queryStart < 0) return stripUrlUserinfo(withoutFragment)
  const base = stripUrlUserinfo(withoutFragment.slice(0, queryStart))
  const query = withoutFragment.slice(queryStart + 1).split('&').map((pair) => {
    const separator = pair.indexOf('=')
    const key = separator < 0 ? pair : pair.slice(0, separator)
    const rawValue = separator < 0 ? '' : pair.slice(separator + 1)
    const decodedKey = decodeQueryName(key)
    return isSensitivePublicName(decodedKey)
      || isMarkedSensitiveName(decodedKey, sensitiveQueryNames)
      || isSensitivePublicValue(rawValue)
      ? `${key}=${PUBLIC_REDACTED}`
      : pair
  }).join('&')
  return `${base}?${query}`
}

function redactHistoryUrl(
  value: string,
  sensitivePathValues: string[],
  sensitiveQueryNames: string[],
): string {
  const redactedPaths = redactLiteralValues(value, sensitivePathValues)
  try {
    const url = new URL(redactedPaths)
    url.username = ''
    url.password = ''
    url.hash = ''
    for (const [key, queryValue] of Array.from(url.searchParams.entries())) {
      if (
        isSensitivePublicName(key)
        || isMarkedSensitiveName(key, sensitiveQueryNames)
        || isSensitivePublicValue(queryValue)
      ) {
        url.searchParams.set(key, PUBLIC_REDACTED)
      }
    }
    return url.toString()
  } catch {
    return redactUnparseableUrl(redactedPaths, sensitiveQueryNames)
  }
}

function toPublicSettings(settings: RequestSettings | undefined): PublicRequestSettings | undefined {
  if (!settings) return undefined
  return {
    timeoutMs: settings.timeoutMs,
    followRedirects: settings.followRedirects,
    maxRedirects: settings.maxRedirects,
    verifyTls: settings.verifyTls,
    httpVersion: settings.httpVersion,
    encodeUrl: settings.encodeUrl,
    sendCookies: settings.sendCookies,
    retryCount: settings.retryCount,
    retryBackoffMs: settings.retryBackoffMs,
  }
}

/** Convert an editable request into the only request-shaped value allowed in history IPC. */
function sensitiveSourceNames(rows: KeyValue[]): string[] {
  const names: string[] = []
  for (const row of rows) {
    const name = row.key.trim()
    if (name && hasSensitiveKeyValue(row) && !isMarkedSensitiveName(name, names)) {
      names.push(name)
    }
  }
  return names
}

function sensitiveAuthQueryNames(auth: AuthConfig | undefined): string[] {
  if (auth?.type !== 'apiKey' || auth.location !== 'query') return []
  const name = auth.key.trim()
  return name ? [name] : []
}

function sensitiveAuthHeaderNames(auth: AuthConfig | undefined): string[] {
  if (auth?.type === 'basic' || auth?.type === 'bearer') return ['Authorization']
  if (auth?.type !== 'apiKey') return []
  if (auth.location === 'cookie') return ['Cookie']
  const name = auth.location === 'header' ? auth.key.trim() : ''
  return name ? [name] : []
}

export function toHistoryRequestPreview(spec: RequestSpec): HistoryRequestPreview {
  const pathParams = spec.pathParams ?? []
  const sensitivePathValues = pathParams
    .filter(hasSensitiveKeyValue)
    .map((row) => row.value)
    .filter(Boolean)
  const sensitiveQueryNames = [
    ...sensitiveSourceNames(spec.params ?? []),
    ...sensitiveAuthQueryNames(spec.auth),
  ]
  const sensitiveHeaderNames = sensitiveAuthHeaderNames(spec.auth)
  return {
    method: spec.method,
    url: redactHistoryUrl(spec.url, sensitivePathValues, sensitiveQueryNames),
    params: (spec.params ?? []).map((row) => toPublicKeyValue(row, sensitiveQueryNames)),
    pathParams: pathParams.map((row) => toPublicKeyValue(row)),
    headers: (spec.headers ?? []).map((row) => toPublicKeyValue(row, sensitiveHeaderNames)),
    settings: toPublicSettings(spec.settings),
  }
}

// ─────────────────────────────────────────────────────────────────────
// 大数据通道：自定义协议 + Range
// ─────────────────────────────────────────────────────────────────────

/**
 * 自定义协议的 scheme 名。
 *
 * ⚠️ 这个字符串有三处必须一致，改一处就要改三处：
 *   1. 这里
 *   2. src-tauri/src/lib.rs 的 `register_asynchronous_uri_scheme_protocol`
 *   3. src-tauri/tauri.conf.json 的 app.security.csp（含 Windows 的
 *      `http://<scheme>.localhost` 形态，两种都要列）
 *
 * 漏掉任一处的表现都是 `TypeError: Failed to fetch` —— 请求被 CSP 静默拦掉，
 * 控制台不报 CSP 违规，很难定位。见 index.html 里的注释。
 */
const PROTOCOL_SCHEME = 'apogee'

/**
 * 响应体的可 Range 读取 URL。
 *
 * **这是整个架构的关键决策（03 文档 §3.1）。**
 * Tauri 的 `invoke` 走 JSON 序列化：10MB body 过一次 invoke 意味着
 * 「Rust 序列化 → 字符串传输 → JS 解析」三次全量拷贝。
 * 走自定义协议 + HTTP Range 语义则是零拷贝语义，且能只取首屏那一段。
 *
 * Windows/Android 上 Tauri 的自定义协议要走 `http://<scheme>.localhost/`；
 * 其它平台是 `<scheme>://`。这里做平台差异归一。
 */
function protocolResponseUrl(responseId: string, query: string): string {
  const isWindows = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)
  const base = isWindows
    ? `http://${PROTOCOL_SCHEME}.localhost/response/${responseId}`
    : `${PROTOCOL_SCHEME}://localhost/response/${responseId}`
  return `${base}?${query}`
}

/** 普通响应 URL；有工作区时必须附带当前 workspace session。 */
export function responseUrl(responseId: string): string {
  // 无工作区的独立 Spike 保持现有调用方式；工作区内的 Spike 必须显式使用
  // `spikeResponseUrl`，避免普通 response view 意外获得 Synthetic scope。
  const query = workspaceSession
    ? `workspaceId=${encodeURIComponent(workspaceSession.workspaceId)}&sessionId=${encodeURIComponent(workspaceSession.sessionId)}`
    : 'scope=spike'
  return protocolResponseUrl(responseId, query)
}

/** Spike 的显式 Synthetic scope URL；它永远不能读取普通 workspace 响应。 */
export function spikeResponseUrl(responseId: string): string {
  return protocolResponseUrl(responseId, 'scope=spike')
}

export interface RangeResult {
  text: string
  /** 本次实际拿到的字节数 */
  bytes: number
  /** 服务端报告的总字节数（来自 Content-Range） */
  totalBytes: number
  /** 纯网络/IPC 耗时，不含解码 */
  fetchMs: number
}

/**
 * 按字节区间读取响应体分片。
 *
 * 用 fetch + Range 头，让 WebView 自己处理传输，前端只拿需要的那一段。
 * CodeMirror 的分段加载就架在这个函数上。
 */
async function fetchResponseRangeAt(
  url: string,
  start: number,
  length: number,
): Promise<RangeResult> {
  const t0 = performance.now()
  const end = start + length - 1
  const res = await fetch(url, {
    headers: { Range: `bytes=${start}-${end}` },
  })
  const buf = await res.arrayBuffer()
  const fetchMs = performance.now() - t0

  // Content-Range: bytes 0-65535/10485760
  const cr = res.headers.get('content-range')
  const totalBytes = cr ? Number(cr.split('/')[1] ?? 0) : buf.byteLength

  return {
    text: new TextDecoder('utf-8').decode(buf),
    bytes: buf.byteLength,
    totalBytes,
    fetchMs,
  }
}

export function fetchResponseRange(
  responseId: string,
  start: number,
  length: number,
): Promise<RangeResult> {
  return fetchResponseRangeAt(responseUrl(responseId), start, length)
}

/** Spike 使用显式 Synthetic scope 读取量测 body，不依赖当前 workspace session。 */
export function fetchSpikeResponseRange(
  responseId: string,
  start: number,
  length: number,
): Promise<RangeResult> {
  return fetchResponseRangeAt(spikeResponseUrl(responseId), start, length)
}

// ─────────────────────────────────────────────────────────────────────
// 窗口控制（自定义标题栏）
//
// `tauri.conf.json` 里 `decorations: false` —— 系统标题栏被关掉了，换来的是
// 02 文档 §3.2 里那条「省掉一条 30px 浪费」。代价是最小化 / 最大化 / 关闭
// 必须自己实现，这一节就是那三个动作的出口。
//
// 为什么放在 IPC facade 里而不是组件里：本文件头部的约定是「组件不允许直接
// import @tauri-apps/api」。窗口控制同样是跨进程调用，同样受这条约束。
// 好处是 Tauri 换版本时只改这一个文件。
//
// 非 Tauri 环境（浏览器里跑 `vite dev`）下全部静默 no-op，标题栏不会报错。
// ─────────────────────────────────────────────────────────────────────

/** 取消监听。与 Tauri 的 UnlistenFn 同形，但不把它的类型泄漏给组件。 */
export type Unlisten = () => void

const appWindow = inTauri ? getCurrentWindow() : null

/** 拖拽缩放时 onResized 每帧都触发，去抖窗口（ms）。 */
const RESIZE_DEBOUNCE_MS = 80

export const windowControls = {
  /** 是否有真实窗口可控。浏览器里为 false，标题栏据此隐藏按钮。 */
  available: appWindow !== null,

  async minimize(): Promise<void> {
    await appWindow?.minimize()
  },

  /** 最大化与还原的切换。与拖拽区双击是同一个语义。 */
  async toggleMaximize(): Promise<void> {
    await appWindow?.toggleMaximize()
  },

  async close(): Promise<void> {
    await appWindow?.close()
  },

  async isMaximized(): Promise<boolean> {
    return (await appWindow?.isMaximized()) ?? false
  },

  /**
   * 订阅最大化状态变化，用于让按钮图标跟上真实窗口状态。
   *
   * 必须订阅而不是只在点击时翻转本地布尔值 —— 用户还可以通过拖到屏幕顶边、
   * 双击拖拽区、`Win`+`↑` 来最大化，这些路径不经过我们的按钮。
   *
   * Tauri 没有「最大化状态变了」这个事件，只能从 resize 事件反查。而 resize
   * 在拖拽缩放时每帧都触发，直接查会变成每帧一次 IPC 往返。所以做尾部去抖：
   * 状态只决定一个图标，晚 80ms 更新看不出来。
   */
  async onMaximizeChange(cb: (maximized: boolean) => void): Promise<Unlisten> {
    const w = appWindow
    if (!w) return () => {}

    let timer: ReturnType<typeof setTimeout> | undefined
    const unlisten = await w.onResized(() => {
      if (timer !== undefined) clearTimeout(timer)
      timer = setTimeout(() => {
        void w.isMaximized().then(cb)
      }, RESIZE_DEBOUNCE_MS)
    })

    return () => {
      if (timer !== undefined) clearTimeout(timer)
      unlisten()
    }
  },
}
