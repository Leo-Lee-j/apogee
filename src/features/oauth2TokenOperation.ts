import { ipc, type AppError, type AuthConfig, type OAuth2TokenFetcher, type RequestSettings } from '@/ipc'
import { useWorkspaceStore } from '@/stores/workspace'

/** Capture ownership BEFORE environment flush or IPC; never recapture a newer workspace. */
export function useOAuth2TokenActions() {
  const workspace = useWorkspaceStore()
  return (nodeId: string | undefined, settings: RequestSettings, canEdit: () => boolean, ownerId = nodeId ?? 'draft', ephemeral = false, before?: () => Promise<boolean>, expectedAuth?: () => AuthConfig | undefined): OAuth2TokenFetcher => {
    const workspaceId = workspace.workspaceId
    const sessionId = workspace.sessionId
    const environmentId = workspace.activeEnvironmentId ?? undefined
    const current = () => canEdit() && workspace.workspaceId === workspaceId && workspace.sessionId === sessionId
      && (workspace.activeEnvironmentId ?? undefined) === environmentId
    const invalid = (): AppError => ({ kind: 'io', message: 'Token 操作所属请求、环境或工作区已变化，操作已取消',
      hints: [{ label: '回到目标请求或配置页后重新获取 Token' }] })
    return async (request, grant, operationId, signal, auth) => {
      if (signal?.aborted) return null
      if (!workspaceId || !sessionId || !current()) throw invalid()
      if (before && !(await before())) throw invalid()
      if (signal?.aborted) return null
      if (!current()) throw invalid()
      if (!(await workspace.saveDirtyEnvironments())) throw workspace.lastError ?? invalid()
      if (signal?.aborted) return null
      if (!current()) throw invalid()
      const response = auth ? (await ipc.oauth2AuthOperation({ operationId, nodeId, ownerId, ephemeral, environmentId,
        auth, expectedAuth: expectedAuth?.(), grant, settings }, { workspaceId, sessionId }))?.token ?? null : await ipc.oauth2FetchToken({ operationId, nodeId, environmentId, request, grant, settings }, { workspaceId, sessionId })
      if (signal?.aborted) return null
      if (!current()) throw invalid()
      return response
    }
  }
}
