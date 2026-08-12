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

export interface Hint {
  label: string
  action?: string
}

export interface AppError {
  kind: ErrorKind
  message: string
  detail?: string
  /** 可操作建议。空数组是设计缺陷，见 02 文档 §8 的三段式错误要求。 */
  hints: Hint[]
}

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

export interface ActualRequest {
  method: string
  url: string
  headers: [string, string][]
  /** 已知浏览器/协议通用 Header 名称，供实际请求表格默认隐藏。 */
  commonHeaderNames?: string[]
  /** 仅小型内存请求体可用；文件和流式上传不会进入快照。 */
  body?: string
}

/**
 * 响应摘要 —— **唯一会过 IPC 的响应信息**，只有几百字节。
 * 响应体本身在磁盘 spool 里，通过 responseUrl 按 Range 拉。
 */
export interface ResponseSummary {
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
  actualRequest?: ActualRequest
  // ── HTTP 语义（03 文档 §3.2）──
  // 全部可选：Spike 合成响应没有 HTTP 语义，UI 据此显示「不可用」而非编造 200。
  status?: number
  statusText?: string
  httpVersion?: string
  headers?: [string, string][]
  charset?: string
  truncated?: boolean
}

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

export interface HistoryEntry {
  id: string
  createdAtMs: number
  method: string
  url: string
  status?: number
  sizeBytes: number
  durationMs: number
}

export interface HistoryRecord {
  id: string
  createdAtMs: number
  spec: RequestSpec
  response: ResponseSummary
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
      detail: `IPC 命令：${cmd}`,
      hints: [{ label: '请从 Apogee 桌面应用启动，而不是直接打开 Vite 预览页' }],
    } satisfies AppError
  }
  try {
    return await tauriInvoke<T>(cmd, args)
  } catch (e) {
    throw normalizeError(e)
  }
}

let workspaceSession: { workspaceId: string; sessionId: string } | null = null

function sessionArgs() {
  if (!workspaceSession) throw new Error('当前没有打开的工作区')
  return workspaceSession
}

export function setWorkspaceSession(session: { workspaceId: string; sessionId: string } | null) {
  workspaceSession = session
}

// ─────────────────────────────────────────────────────────────────────
// 命令（03 文档 §3.2）
// ─────────────────────────────────────────────────────────────────────

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
    return call('get_response_meta', { responseId })
  },

  /** 按路径取一页树节点。path 为空字符串表示根。 */
  getResponseTree(
    responseId: string,
    path: string,
    offset: number,
    limit: number,
  ): Promise<TreeNode[]> {
    return call('get_response_tree', { responseId, path, offset, limit })
  },

  /** 让 Rust 侧解析 JSON 并建索引。大 body 下这一步是显式的、可跳过的。 */
  indexResponse(responseId: string): Promise<ResponseSummary> {
    return call('index_response', { responseId })
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
    await call('workspace_close', session)
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
    return call('workspace_load', sessionArgs())
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
  sendRequest(spec: RequestSpec, ctx?: ExecContext): Promise<ExecHandle> {
    return call('send_request', { ...sessionArgs(), spec, ctx })
  },

  /** 取消一个正在执行的请求。返回 false 表示请求已结束或不存在。 */
  cancelRequest(handleId: string): Promise<boolean> {
    return call('cancel_request', { handleId })
  },

  historyRecord(createdAtMs: number, spec: RequestSpec, response: ResponseSummary): Promise<void> {
    return call('history_record', { ...sessionArgs(), createdAtMs, spec, response })
  },

  historyList(): Promise<HistoryEntry[]> {
    return call('history_list', sessionArgs())
  },

  historyRead(historyId: string): Promise<HistoryRecord> {
    return call('history_read', { ...sessionArgs(), historyId })
  },

  historyClear(): Promise<void> {
    return call('history_clear', sessionArgs())
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
  postmanImportWorkspace(path: string): Promise<{ collectionId: string; importedRequests: number; importedFolders: number; warnings: string[] }> {
    return call('postman_import_workspace', { ...sessionArgs(), path })
  },
  postmanImportEnvironmentWorkspace(path: string): Promise<{ environment: Environment; warnings: string[] }> {
    return call('postman_import_environment_workspace', { ...sessionArgs(), path })
  },
}

// ─────────────────────────────────────────────────────────────────────
// 请求事件（03 文档 §3.2 / §4.5）
// ─────────────────────────────────────────────────────────────────────

import { listen as tauriListen } from '@tauri-apps/api/event'

export interface ProgressEvent {
  id: string
  received: number
  total: number | null
}

export interface DoneEvent {
  response: ResponseSummary
}

export interface ErrorEvent {
  id: string
  error: AppError
}

export interface CancelledEvent {
  id: string
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

/** 请求级设置。全部可选：不传 = 继承默认。 */
export interface RequestSettings {
  timeoutMs?: number
  followRedirects?: boolean
  maxRedirects?: number
  verifyTls?: boolean
  httpVersion?: 'http1' | 'http2'
  encodeUrl?: boolean
  sendCookies?: boolean
}

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
export function responseUrl(responseId: string): string {
  const isWindows = typeof navigator !== 'undefined' && /Windows/i.test(navigator.userAgent)
  return isWindows
    ? `http://${PROTOCOL_SCHEME}.localhost/response/${responseId}`
    : `${PROTOCOL_SCHEME}://localhost/response/${responseId}`
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
export async function fetchResponseRange(
  responseId: string,
  start: number,
  length: number,
): Promise<RangeResult> {
  const t0 = performance.now()
  const end = start + length - 1
  const res = await fetch(responseUrl(responseId), {
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
