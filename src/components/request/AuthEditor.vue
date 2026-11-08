<script setup lang="ts">
import { computed, ref, watch, onBeforeUnmount } from 'vue'
import { ipc, normalizeError, type AuthConfig, type AppError, type OAuth2Grant, type OAuth2TokenFetcher, type OAuth2TokenRequest } from '@/ipc'
import { AppInput, AppSelect, AppButton, AppAlert, AppSwitch } from '@/components/ui'
import { AUTH_OPTIONS, authFields, selectAuth, defaultTokenRequest, TOKEN_REQUEST_FIELDS, PKCE_TOKEN_FIELDS, oauth2OperationError } from '@/features/requestAuth'

import { applyTokenResponse, tokenState } from '@/features/oauth2Lifecycle'

const props = withDefaults(defineProps<{ value: AuthConfig; allowInherit?: boolean; allowAutoRefresh?: boolean; inheritLabel?: string; canEdit?: () => boolean; fetchToken?: OAuth2TokenFetcher }>(), {
  allowInherit: true, inheritLabel: '继承集合或文件夹',
})
const emit = defineEmits<{ 'update:value': [value: AuthConfig]; 'token-updated': [value: AuthConfig] }>()
const options = computed(() => AUTH_OPTIONS.filter(o => props.allowInherit || o.value !== 'inherit')
  .map(o => o.value === 'inherit' ? { ...o, label: props.inheritLabel } : o))
const selectValue = computed(() => !props.allowInherit && props.value.type === 'inherit' ? 'none' : props.value.type)
const fields = computed(() => authFields(props.value))
function setType(type: string | number | null) {
  if (props.canEdit && !props.canEdit()) return
  const option = options.value.find(o => o.value === type)
  if (!option) return
  const next = selectAuth(option.value, props.value)
  if (next !== props.value) emit('update:value', next)
}
function fieldValue(key: string): string {
  return String((props.value as unknown as Record<string, unknown>)[key] ?? '')
}
// Capture the rendered scheme: a blur event from an old field must not pollute a new scheme.
function fieldUpdater(expected: AuthConfig, key: string) {
  return (value: string | number | null) => {
    if ((props.canEdit && !props.canEdit()) || props.value.type !== expected.type || (expected.type === 'oauth2' && props.value !== expected)) return
    const next = String(value ?? '')
    if (fieldValue(key) !== next) {
      const updated = { ...props.value, [key]: next } as AuthConfig
      if (updated.type === 'oauth2' && key === 'accessToken') { delete updated.obtainedAt; delete updated.expiresAt; delete updated.tokenContext }
      emit('update:value', updated)
    }
  }
}
const tokenGrant = computed(() => props.value.type === 'oauth2' ? props.value.tokenRequest?.grantType ?? 'clientCredentials' : 'clientCredentials')
const tokenFields = computed(() => tokenGrant.value === 'authorizationCode' ? [...PKCE_TOKEN_FIELDS, ...TOKEN_REQUEST_FIELDS] : TOKEN_REQUEST_FIELDS)
function grantUpdater(expected: AuthConfig) {
  return (value: string | number | null) => {
    if (value !== 'clientCredentials' && value !== 'authorizationCode') return
    if (props.value !== expected || (props.canEdit && !props.canEdit()) || props.value.type !== 'oauth2' || !props.value.tokenRequest) return
    if ((props.value.tokenRequest.grantType ?? 'clientCredentials') === value) return
    emit('update:value', { ...props.value, tokenRequest: { ...props.value.tokenRequest, grantType: value,
      ...(value === 'authorizationCode' && !props.value.tokenRequest.redirectUri ? { redirectUri: 'http://127.0.0.1:53682/oauth/callback' } : {}) } })
  }
}
const clock = ref(Date.now())
let clockTimer: ReturnType<typeof setInterval> | undefined
watch(() => props.value.type, type => {
  if (clockTimer !== undefined) clearInterval(clockTimer)
  clockTimer = undefined
  if (type === 'oauth2') { clock.value = Date.now(); clockTimer = setInterval(() => { clock.value = Date.now() }, 1000) }
}, { immediate: true })
onBeforeUnmount(() => { if (clockTimer !== undefined) clearInterval(clockTimer) })
const expiryStatus = computed(() => props.value.type === 'oauth2' ? {
  missing: '尚无 Access Token', unknown: 'Token 有效期未知（手动粘贴或服务未返回 expires_in）',
  valid: 'Token 尚未过期', expiring: 'Token 将在 30 秒内过期', expired: 'Token 已过期',
  clockInvalid: '系统时间早于 Token 获取时间，请校准时间或重新获取',
}[tokenState(props.value, clock.value)] : '')
function autoRefreshUpdater(expected: AuthConfig) {
  return (value: boolean) => {
    if (!props.allowAutoRefresh || props.value !== expected || expected.type !== 'oauth2' || (props.canEdit && !props.canEdit())) return
    if (Boolean(expected.autoRefresh) !== value) emit('update:value', { ...expected, autoRefresh: value })
  }
}
const tokenBusy = ref(false)
const tokenError = ref<AppError | null>(null)
const tokenStatus = ref('')
let operation: { id: string; auth: AuthConfig; controller: AbortController } | null = null
function cancelToken() {
  if (!operation) return
  const id = operation.id
  operation.controller.abort()
  operation = null
  tokenBusy.value = false
  tokenStatus.value = 'Token 操作已取消'
  void ipc.cancelOAuth2Token(id).catch(() => {})
}
watch(() => props.value, value => { if (operation && operation.auth !== value) cancelToken() })
watch(() => props.fetchToken, () => { if (operation) cancelToken() })
watch(() => props.canEdit?.(), allowed => { if (allowed === false) cancelToken() })
onBeforeUnmount(cancelToken)
function configureToken() {
  if ((props.canEdit && !props.canEdit()) || props.value.type !== 'oauth2') return
  emit('update:value', { ...props.value, tokenRequest: defaultTokenRequest() })
}
function tokenFieldUpdater(expected: AuthConfig, key: keyof OAuth2TokenRequest) {
  return (value: string | number | null) => {
    if ((props.canEdit && !props.canEdit()) || props.value !== expected || props.value.type !== 'oauth2' || expected.type !== 'oauth2' || !props.value.tokenRequest) return
    const next = String(value ?? '')
    if (props.value.tokenRequest[key] !== next) emit('update:value', { ...props.value, tokenRequest: { ...props.value.tokenRequest, [key]: next } })
  }
}
async function fetchToken(grant: OAuth2Grant) {
  if (tokenBusy.value || !props.fetchToken || props.value.type !== 'oauth2' || !props.value.tokenRequest || (props.canEdit && !props.canEdit())) return
  const initial = props.value
  const current = { id: crypto.randomUUID(), auth: initial, controller: new AbortController() }
  operation = current
  tokenBusy.value = true; tokenError.value = null; tokenStatus.value = grant === 'authorizationCode' ? '等待系统浏览器授权（最长 3 分钟）' : ''
  try {
    const startedAt = Date.now()
    const response = await props.fetchToken({ ...initial.tokenRequest! }, grant, current.id, current.controller.signal, initial)
    if (operation !== current || props.value !== initial || (props.canEdit && !props.canEdit())) return
    operation = null
    if (!response) { tokenStatus.value = 'Token 操作已取消'; return }
    clock.value = Date.now()
    const next = applyTokenResponse(initial, response, startedAt)
    emit('update:value', next)
    emit('token-updated', next)
    tokenStatus.value = response.expiresIn == null ? 'Token 已更新' : `Token 已更新，有效期 ${response.expiresIn} 秒`
  } catch (cause) {
    if (operation === current && props.value === initial && (!props.canEdit || props.canEdit())) tokenError.value = oauth2OperationError(normalizeError(cause))
  } finally {
    if (operation === current) operation = null
    if (!operation) tokenBusy.value = false
  }
}
</script>

<template>
  <div class="auth-fields">
    <label class="auth-field"><span>鉴权类型</span><AppSelect :value="selectValue" :options="options" @update:value="setType" /></label>
    <p v-if="value.type === 'inherit'" class="auth-note">继承最近集合或文件夹的鉴权配置；未配置上级时等同无鉴权。上级 OAuth2 开启发送前自动刷新时，更新写回真实来源，不复制到子请求。</p>
    <p v-else-if="value.type === 'none'" class="auth-note">此请求不附加鉴权信息。</p>
    <label v-for="field in fields" :key="`${value.type}:${field.key}`" class="auth-field">
      <span>{{ field.label }}</span>
      <AppInput :value="fieldValue(field.key)" :type="field.secret ? 'password' : 'text'"
        show-password-on="click" autocomplete="off" :placeholder="field.placeholder"
        v-bind="{ 'onUpdate:value': fieldUpdater(value, field.key) }" />
    </label>
    <label v-if="value.type === 'apiKey'" class="auth-field"><span>添加到</span><AppSelect :value="value.location"
      :options="[{ label: 'Header', value: 'header' }, { label: 'Query', value: 'query' }, { label: 'Cookie', value: 'cookie' }]"
      v-bind="{ 'onUpdate:value': fieldUpdater(value, 'location') }" /></label>
    <template v-else-if="value.type === 'oauth1'">
      <label class="auth-field"><span>签名算法</span><AppSelect :value="value.signatureMethod"
        :options="['HMAC-SHA1', 'HMAC-SHA256'].map(value => ({ label: value, value }))"
        v-bind="{ 'onUpdate:value': fieldUpdater(value, 'signatureMethod') }" /></label>
      <p class="auth-note">发送时签名最终 URL 与 urlencoded Body，并写入 Authorization Header。协议参数仅在此配置；不自动跟随重定向。Nonce / Timestamp 留空适用于正常请求，固定值仅用于调试。</p>
    </template>
    <template v-else-if="value.type === 'oauth2'">
      <p class="auth-note">可粘贴已有 Access Token，或显式获取 / 刷新 Token。Token URL 需为 HTTPS（本机调试可用 HTTP）；授权码使用系统浏览器及 PKCE S256。</p>
      <p class="auth-note" role="status">{{ expiryStatus }}</p>
      <label v-if="allowAutoRefresh" class="auth-field"><span>发送前自动刷新（默认关闭）</span>
        <AppSwitch :value="value.autoRefresh === true" v-bind="{ 'onUpdate:value': autoRefreshUpdater(value) }" />
      </label>
      <p class="auth-note">自动刷新用于桌面发送及继承此配置的请求；有效期未知时不推测。已知期限提前 30 秒刷新，签发环境或网络配置改变时重新获取；失败则不发送。不在 401 后重放，不后台定时刷新或静默打开浏览器。</p>
      <AppButton v-if="!value.tokenRequest" class="token-action" quaternary @click="configureToken">配置 Token 获取</AppButton>
      <template v-else>
        <label class="auth-field"><span>Token 授权方式</span><AppSelect :value="tokenGrant"
          :options="[{ label: 'Client Credentials', value: 'clientCredentials' }, { label: '授权码 + PKCE（S256）', value: 'authorizationCode' }]"
          v-bind="{ 'onUpdate:value': grantUpdater(value) }" /></label>
        <p v-if="tokenGrant === 'authorizationCode'" class="auth-note">Redirect URI 必须与认证服务注册值一致，仅支持带固定端口的 http://127.0.0.1 回调。公共客户端可留空 Client Secret。取消只停止等待，不关闭已打开的浏览器。</p>
        <label v-for="field in tokenFields" :key="`token:${field.key}`" class="auth-field">
          <span>{{ field.label }}</span><AppInput :value="value.tokenRequest[field.key as keyof OAuth2TokenRequest]"
            :type="field.secret ? 'password' : 'text'" show-password-on="click" autocomplete="off" :placeholder="field.placeholder"
            v-bind="{ 'onUpdate:value': tokenFieldUpdater(value, field.key as keyof OAuth2TokenRequest) }" />
        </label>
        <label class="auth-field"><span>客户端凭据位置</span><AppSelect :value="value.tokenRequest.clientAuth"
          :options="[{ label: 'Basic Auth Header', value: 'basic' }, { label: '请求体', value: 'body' }]"
          v-bind="{ 'onUpdate:value': tokenFieldUpdater(value, 'clientAuth') }" /></label>
        <div class="token-actions">
          <AppButton :disabled="tokenBusy || !props.fetchToken" @click="fetchToken(tokenGrant)">{{ tokenGrant === 'authorizationCode' ? '浏览器授权获取 Token' : '获取 Token' }}</AppButton>
          <AppButton :disabled="tokenBusy || !props.fetchToken || !value.tokenRequest.refreshToken" @click="fetchToken('refreshToken')">刷新 Token</AppButton>
          <AppButton v-if="tokenBusy" quaternary @click="cancelToken">取消</AppButton>
        </div>
        <p v-if="tokenStatus" class="auth-note" role="status">{{ tokenStatus }}</p>
        <AppAlert v-if="tokenError" class="token-feedback" type="error" :title="tokenError.message">
          <p v-for="hint in tokenError.hints" :key="hint.label">{{ hint.label }}</p>
        </AppAlert>
        <p class="auth-note">此操作不进入普通响应、历史或 spool；已保存来源的 Token 更新直接写回请求 / 集合 / 文件夹，草稿仅回填当前配置。手动与自动操作按真实来源串行；来源变更时停止，不覆盖新凭据。配置变更或离开编辑器会取消操作。凭据保存在本地配置中。草稿 / 用例的凭据更新走此专用认证通道，不通过普通发送响应回传，也不写入基础请求。</p>
      </template>
    </template>
    <template v-else-if="value.type === 'jwt'">
      <label class="auth-field"><span>签名算法</span><AppSelect :value="value.algorithm"
        :options="['HS256', 'HS384', 'HS512'].map(value => ({ label: value, value }))"
        v-bind="{ 'onUpdate:value': fieldUpdater(value, 'algorithm') }" /></label>
      <p class="auth-note">每次发送生成 HMAC JWT。Secret 按 UTF-8 使用；Payload / Header 必须是 JSON 对象，可使用环境变量。exp / iat 等声明由你配置；不支持 RSA / ECDSA。</p>
    </template>
    <p v-if="value.type !== 'none' && value.type !== 'inherit'" class="auth-note">所有字段支持 <code v-text="'{{变量名}}'" />。请使用 HTTPS；手写 Authorization Header 优先于此配置。凭据保存在本地请求配置，响应快照、历史和文档会脱敏。</p>
  </div>
</template>

<style scoped>
.auth-fields { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; width: 100%; }
.token-actions, .token-action, .token-feedback { grid-column: 1 / -1; }
.token-actions { display: flex; gap: 8px; flex-wrap: wrap; }
.auth-field { display: flex; flex-direction: column; gap: 6px; min-width: 0; font-size: var(--a-fs-sm); }
.auth-note { grid-column: 1 / -1; margin: 0; color: var(--a-text-dim); font-size: var(--a-fs-xs); line-height: var(--a-lh); }
@media (max-width: 640px) { .auth-fields { grid-template-columns: minmax(0, 1fr); } }
</style>
