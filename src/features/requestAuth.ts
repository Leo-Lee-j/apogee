import type { AppError, AuthConfig, OAuth2TokenRequest } from '@/ipc'

export const AUTH_OPTIONS: { value: AuthConfig['type']; label: string }[] = [
  { value: 'inherit', label: '继承' }, { value: 'none', label: '无鉴权' },
  { value: 'basic', label: 'Basic Auth' }, { value: 'bearer', label: 'Bearer Token' },
  { value: 'apiKey', label: 'API Key' }, { value: 'oauth1', label: 'OAuth 1.0' },
  { value: 'oauth2', label: 'OAuth 2.0' }, { value: 'jwt', label: 'JWT' },
]

export function authSummary(auth: AuthConfig): string {
  return AUTH_OPTIONS.find(option => option.value === auth.type)?.label ?? '无鉴权'
}

/** Switching schemes never carries unrelated credential fields into the next model. */
export function selectAuth(type: AuthConfig['type'], current: AuthConfig): AuthConfig {
  if (current.type === type) return current
  switch (type) {
    case 'inherit': case 'none': return { type }
    case 'basic': return { type, username: '', password: '' }
    case 'bearer': return { type, token: '' }
    case 'apiKey': return { type, key: '', value: '', location: 'header' }
    case 'oauth1': return { type, consumerKey: '', consumerSecret: '', token: '', tokenSecret: '',
      signatureMethod: 'HMAC-SHA1', realm: '', callback: '', verifier: '', nonce: '', timestamp: '' }
    case 'oauth2': return { type, accessToken: '', headerPrefix: 'Bearer' }
    case 'jwt': return { type, secret: '', payload: '{}', header: '{}', algorithm: 'HS256', headerPrefix: 'Bearer' }
  }
}

export interface AuthField { key: string; label: string; secret?: boolean; placeholder?: string }
export function authFields(auth: AuthConfig): AuthField[] {
  switch (auth.type) {
    case 'inherit': case 'none': return []
    case 'basic': return [{ key: 'username', label: '用户名' }, { key: 'password', label: '密码', secret: true }]
    case 'bearer': return [{ key: 'token', label: 'Token', secret: true }]
    case 'apiKey': return [{ key: 'key', label: 'Key' }, { key: 'value', label: 'Value', secret: true }]
    case 'oauth2': return [{ key: 'accessToken', label: 'Access Token', secret: true },
      { key: 'headerPrefix', label: 'Header Prefix', placeholder: 'Bearer（清空可直接发送 Token）' }]
    case 'jwt': return [{ key: 'secret', label: 'JWT Secret（UTF-8）', secret: true },
      { key: 'payload', label: 'Payload（JSON 对象）', secret: true },
      { key: 'header', label: 'Header（JSON 对象，可配置 kid）', secret: true },
      { key: 'headerPrefix', label: 'Header Prefix', placeholder: 'Bearer（清空可直接发送 Token）' }]
    case 'oauth1': return [
      { key: 'consumerKey', label: 'Consumer Key' }, { key: 'consumerSecret', label: 'Consumer Secret', secret: true },
      { key: 'token', label: 'Access Token（可选）', secret: true }, { key: 'tokenSecret', label: 'Token Secret（可选）', secret: true },
      { key: 'realm', label: 'Realm（可选）' }, { key: 'callback', label: 'Callback URL（可选）' },
      { key: 'verifier', label: 'Verifier（可选）', secret: true },
      { key: 'nonce', label: 'Nonce（可选）', placeholder: '留空：每次发送自动生成' },
      { key: 'timestamp', label: 'Timestamp（Unix 秒，可选）', placeholder: '留空：使用当前时间' },
    ]
  }
}

export function defaultTokenRequest(): OAuth2TokenRequest {
  return { tokenUrl: '', clientId: '', clientSecret: '', scope: '', refreshToken: '', clientAuth: 'basic' }
}
export const TOKEN_REQUEST_FIELDS: AuthField[] = [
  { key: 'tokenUrl', label: 'Token URL', placeholder: 'https://auth.example.com/oauth/token' },
  { key: 'clientId', label: 'Client ID' }, { key: 'clientSecret', label: 'Client Secret', secret: true },
  { key: 'scope', label: 'Scope（可选）', placeholder: '多个 scope 以空格分隔' },
  { key: 'refreshToken', label: 'Refresh Token（可选，刷新时必填）', secret: true },
]

export const PKCE_TOKEN_FIELDS: AuthField[] = [
  { key: 'authorizationUrl', label: 'Authorization URL', placeholder: 'https://auth.example.com/oauth/authorize' },
  { key: 'redirectUri', label: 'Redirect URI（需在认证服务注册）', placeholder: 'http://127.0.0.1:53682/oauth/callback' },
]

/** Explicit collection export only; never use this projection for history / documentation. */
export function postmanAuth(auth: AuthConfig | undefined): Record<string, unknown> | undefined {
  if (!auth || auth.type === 'inherit') return undefined
  if (auth.type === 'none') return { type: 'noauth' }
  const type = auth.type === 'apiKey' ? 'apikey' : auth.type
  let fields: Record<string, string>
  switch (auth.type) {
    case 'basic': fields = { username: auth.username, password: auth.password }; break
    case 'bearer': fields = { token: auth.token }; break
    case 'apiKey': fields = { key: auth.key, value: auth.value, in: auth.location }; break
    case 'oauth1': fields = { consumerKey: auth.consumerKey, consumerSecret: auth.consumerSecret, token: auth.token,
      tokenSecret: auth.tokenSecret, signatureMethod: auth.signatureMethod, realm: auth.realm,
      callback: auth.callback, verifier: auth.verifier, nonce: auth.nonce, timestamp: auth.timestamp,
      addParamsToHeader: 'true', addEmptyParamsToSign: 'false' }; break
    case 'oauth2':
      fields = { accessToken: auth.accessToken, headerPrefix: auth.headerPrefix, addTokenTo: 'header' }
      if (auth.tokenRequest) Object.assign(fields, { grant_type: auth.tokenRequest.grantType === 'authorizationCode' ? 'authorization_code_with_pkce' : 'client_credentials',
        accessTokenUrl: auth.tokenRequest.tokenUrl,
        clientId: auth.tokenRequest.clientId, clientSecret: auth.tokenRequest.clientSecret, scope: auth.tokenRequest.scope,
        refreshToken: auth.tokenRequest.refreshToken, client_authentication: auth.tokenRequest.clientAuth === 'basic' ? 'header' : 'body' })
      if (auth.tokenRequest?.grantType === 'authorizationCode') Object.assign(fields, { authUrl: auth.tokenRequest.authorizationUrl ?? '',
        redirect_uri: auth.tokenRequest.redirectUri ?? '', code_challenge_method: 'S256' })
      break
    case 'jwt': fields = { secret: auth.secret, payload: auth.payload, header: auth.header,
      algorithm: auth.algorithm, headerPrefix: auth.headerPrefix, isSecretBase64Encoded: 'false', addTokenTo: 'header' }; break
  }
  return { type, [type]: Object.entries(fields).map(([key, value]) => ({ key, value, type: 'string' })) }
}

/** Map only static core codes; never restore backend free-text error details. */
export function oauth2OperationError(error: AppError): AppError {
  const messages: Record<string, string> = {
    OAUTH2_REDIRECT_INVALID: 'Redirect URI 需使用带固定端口和路径的 http://127.0.0.1，并与认证服务注册值一致',
    OAUTH2_AUTH_URL_INVALID: 'Authorization URL 需使用完整 HTTPS URL（本机调试可 HTTP），不能含凭据或 fragment',
    OAUTH2_AUTH_PARAMS_INVALID: 'Authorization URL 含冲突协议参数，请通过 Auth 配置填写 Client ID、Scope 和回调',
    OAUTH2_CALLBACK_BIND_FAILED: 'OAuth2 回调端口无法绑定，请检查端口占用；修改端口后需重新注册 Redirect URI',
    OAUTH2_BROWSER_OPEN_FAILED: '无法打开系统浏览器，请检查默认浏览器配置后重试',
    OAUTH2_AUTH_DENIED: '浏览器授权被拒绝或失败，未交换 Token',
    OAUTH2_AUTH_TIMEOUT: '浏览器授权超过 3 分钟，已停止等待，请重新授权',
    OAUTH2_CALLBACK_LIMIT: '收到过多无效回调，授权已停止，请重新授权',
    OAUTH2_CALLBACK_IO: 'OAuth2 本机回调监听失败，请检查本机网络后重试',
    OAUTH2_CONFIG_TOO_LARGE: 'OAuth2 授权配置过大，请减少 URL 参数或 Scope 长度',
    OAUTH2_NETWORK_INVALID: 'OAuth2 网络配置无效，请检查 TLS 和代理设置',
    OAUTH2_RANDOM_FAILED: '无法生成安全随机数，授权已停止，请重试',
    OAUTH2_SESSION_STALE: '授权所属工作区会话已失效，结果已丢弃',
    OAUTH2_SOURCE_STALE: '认证来源、凭据、环境或网络设置已更新；请重新加载配置后重试，旧结果未回写',
    OAUTH2_SOURCE_WRITE_FAILED: 'Token 已返回但来源配置保存失败；请检查磁盘和工作区权限，刷新凭据失效时需重新授权',
    OAUTH2_SOURCE_BUSY: '认证操作排队或并发数量达到上限，请取消不用的授权操作后重试',
    OAUTH2_CONFIG_REQUIRED: '请先在认证来源中配置 OAuth2 Token 获取，或关闭发送前自动刷新',
    OAUTH2_EPHEMERAL_PREPARATION_REQUIRED: '草稿 / 用例的认证需要先通过 Auth 更新 Token；未发送请求，也未丢弃或覆盖旋转凭据',
    OAUTH2_REAUTHORIZE: '授权码 Token 没有 Refresh Token，请在认证来源中重新进行浏览器授权',
    OAUTH2_TOKEN_TOO_SHORT: '新 Token 已过期或有效期不足 30 秒；已保留旋转凭据，请检查认证服务',
    REQUEST_PREPARATION_CONTEXT_INVALID: '请求执行上下文无效，请重新加载请求后重试；未发送业务请求',
    REQUEST_PREPARATION_SOURCE_STALE: '请求来源或环境已失效，请重新加载后重试；未发送业务请求',
    REQUEST_PREPARATION_FAILED: '请求认证准备失败，请检查 Auth 和环境配置；未发送业务请求',
    OAUTH2_OPERATION_INVALID: '授权所属请求、环境或工作区无效，请回到目标配置后重试',
  }
  const message = error.code && Object.hasOwn(messages, error.code) ? messages[error.code] : undefined
  return message ? { ...error, message } : error
}
