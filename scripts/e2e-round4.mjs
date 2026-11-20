import assert from 'node:assert/strict'
import { readFileSync, readdirSync } from 'node:fs'
import { join, relative } from 'node:path'

// All values are synthetic. The caller deletes the isolated workspace through IPC in finally.
export async function round4(page, root, base, received) {
  const fixture = await page.evaluate(async base => {
    const { ipc } = await import('/src/ipc/index.ts')
    const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
    const workspace = useWorkspaceStore()
    const snapshot = await ipc.workspaceCreateCollection('Round4 Auth vault')
    const collection = snapshot.nodes.find(n => n.name === 'Round4 Auth vault')
    let request = await ipc.workspaceCreateRequest(collection.id, 'Round4 credentials')
    const marker = 'round4-synthetic-credential'
    const auths = [
      { type: 'basic', username: marker, password: marker },
      { type: 'bearer', token: marker.repeat(300) },
      { type: 'oauth1', consumerKey: marker, consumerSecret: marker, token: marker, tokenSecret: marker, verifier: marker },
      { type: 'jwt', secret: marker, payload: JSON.stringify({ sub: marker }) },
      { type: 'apiKey', key: 'X-Key', value: marker, location: 'header' },
    ]
    for (const auth of auths) {
      await ipc.workspaceSaveRequest(request.id, request.name, { ...request.spec, auth }, request.spec.auth)
      request = await ipc.workspaceReadRequest(request.id)
      if (request.spec.auth.type !== auth.type) throw new Error('Auth type did not roundtrip')
      for (const [field, value] of Object.entries(auth)) {
        if (request.spec.auth[field] !== value) throw new Error('Vault Auth field did not roundtrip: ' + field)
      }
      await ipc.workspaceCreateDebugCase(request.id, auth.type + ' case', 'success', request.spec)
    }
    const variable = (key, value, secret = false) => ({ id: crypto.randomUUID(), key, value, secret, enabled: true })
    await ipc.workspaceSaveGlobalVariables([
      variable('endpoint', base + '/round4/token'), variable('client', 'global-client'), variable('secret', 'round4-synthetic-client-secret', true),
    ])
    const oauth2 = { type: 'oauth2', accessToken: '', headerPrefix: 'Bearer', autoRefresh: true,
      tokenRequest: { tokenUrl: '{{endpoint}}', clientId: '{{client}}', clientSecret: '{{secret}}', refreshToken: '', scope: '', clientAuth: 'body', grantType: 'clientCredentials', authorizationUrl: '', redirectUri: '' } }
    await ipc.workspaceSaveRequest(request.id, request.name, { ...request.spec, auth: oauth2 }, request.spec.auth)
    const output = await ipc.oauth2AuthOperation({ operationId: crypto.randomUUID(), nodeId: request.id, ownerId: request.id,
      auth: oauth2, expectedAuth: oauth2, grant: 'clientCredentials', settings: {}, runtimeVariables: { client: 'runtime-client' } })
    if (!output?.token || output.auth.accessToken !== 'round4-synthetic-access') throw new Error('Scoped Token acquisition did not complete')
    const saved = await ipc.workspaceReadRequest(request.id)
    const copied = await ipc.workspaceDuplicateNode(collection.id)
    const copy = copied.nodes.find(n => n.name === 'Round4 Auth vault 副本')
    const copiedRequest = copied.nodes.find(n => n.parentId === copy.id && n.kind === 'request')
    const copiedAuth = (await ipc.workspaceReadRequest(copiedRequest.id)).spec.auth
    const exported = await ipc.workspaceExportPostman(collection.id)
    await workspace.load()
    workspace.selectEnvironment(null)
    workspace.addGlobalVariable()
    workspace.updateGlobalVariable(workspace.globalVariables.length - 1, { key: 'round4Ui', value: 'pending' })
    return { collectionId: collection.id, saved, copiedAuth, exported, path: workspace.descriptor.path }
  }, base)
  assert.equal(fixture.saved.spec.auth.accessToken, 'round4-synthetic-access')
  assert.equal(fixture.saved.spec.auth.tokenRequest.refreshToken, 'round4-synthetic-refresh')
  assert.equal(fixture.copiedAuth.accessToken, 'round4-synthetic-access')
  assert.ok(!fixture.exported.json.includes('round4-synthetic-'))
  assert.ok(fixture.exported.json.includes('{{secret}}'))
  assert.ok(fixture.exported.warnings.some(w => w.includes('凭据')))
  const token = received.findLast(r => r.url === '/round4/token')
  assert.ok(token, 'Token endpoint did not receive the request')
  assert.equal(new URLSearchParams(token.body).get('client_id'), 'runtime-client')
  assert.equal(new URLSearchParams(token.body).get('client_secret'), 'round4-synthetic-client-secret')
  function inspect(directory) {
    for (const item of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, item.name)
      if (item.isDirectory()) inspect(path)
      else if (item.name.endsWith('.yaml')) {
        const yaml = readFileSync(path, 'utf8')
        assert.ok(!yaml.includes('round4-synthetic-'), 'Credential leaked into workspace YAML')
      }
    }
  }
  const workspacePath = relative(root, fixture.path)
  assert.ok(workspacePath && !workspacePath.startsWith('..'), 'fixture must be under the isolated E2E root')
  inspect(join(fixture.path, 'collections'))
  await page.locator('.variables-button .dirty-dot').waitFor()
  await page.locator('.variables-button').click()
  const modal = page.locator('.environment-modal')
  await modal.getByRole('status').filter({ hasText: '有未保存修改' }).waitFor()
  await modal.getByRole('button', { name: '完成', exact: true }).click()
  await page.locator('.variables-button .dirty-dot').waitFor({ state: 'hidden' })
  console.log('E2E Round4 PASS: real Windows Auth vault (including large values/cases/copy/Token rotation), safe export, scoped Token exchange and global-only dirty UI')
}
