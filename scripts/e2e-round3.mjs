import assert from 'node:assert/strict'
import { existsSync, readFileSync } from 'node:fs'
import { join, relative } from 'node:path'

// Use synthetic credentials only; finally deletes every environment/global entry created here.
export async function round3(page, root, base, received) {
  const fixture = await page.evaluate(async base => {
    const { ipc } = await import('/src/ipc/index.ts')
    const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
    const workspace = useWorkspaceStore()
    const snapshot = await ipc.workspaceCreateCollection('Round3 variable scopes')
    const collection = snapshot.nodes.find(node => node.name === 'Round3 variable scopes')
    await ipc.workspaceCreateFolder(collection.id, 'Round3 folder')
    const folder = (await ipc.workspaceLoad()).nodes.find(node => node.name === 'Round3 folder')
    const variable = (key, value, secret = false) => ({ id: crypto.randomUUID(), key, value, secret, enabled: true })
    for (const [id, variables] of [
      [collection.id, [variable('collectionOnly', 'collection'), variable('shared', 'collection')]],
      [folder.id, [variable('folderOnly', 'folder'), variable('shared', 'folder')]],
    ]) {
      const container = await ipc.workspaceReadContainer(id)
      await ipc.workspaceSaveContainer({ ...container, variables }, container.auth)
    }
    let environmentId
    try {
      await ipc.workspaceSaveGlobalVariables([
        variable('globalOnly', 'global'), variable('shared', 'global'),
        variable('globalPrivate', 'round3-synthetic-global', true),
      ])
      let environment = await ipc.workspaceCreateEnvironment('Round3 secure environment')
      environmentId = environment.id
      environment = await ipc.workspaceSaveEnvironment({ ...environment, host: base, variables: [
        variable('private', 'round3-synthetic-environment', true), variable('shared', 'environment'),
      ] })
      const request = await ipc.workspaceCreateRequest(folder.id, 'Round3 scoped request')
      const row = (key, value) => ({ key, value, enabled: true })
      const spec = { ...request.spec, method: 'GET', url: '{{env:host}}/round3/{{env:private}}',
        auth: { type: 'none' }, headers: [row('X-Correlation', '{{private}}'), row('X-Global-Private', '{{globalPrivate}}'),
          row('X-Global', '{{globalOnly}}'), row('X-Collection', '{{collectionOnly}}'), row('X-Folder', '{{folderOnly}}'), row('X-Shared', '{{shared}}')],
        params: [row('trace', '{{private}}')], pathParams: [], body: { type: 'none' } }
      await ipc.workspaceSaveRequest(request.id, request.name, spec, request.spec.auth)
      await workspace.load()
      workspace.selectEnvironment(environment.id)
      await workspace.openRequest({ id: request.id, kind: 'request' }, false)
      return { workspaceId: workspace.workspaceId, environmentId: environment.id, requestId: request.id, path: workspace.descriptor.path, environment }
    } catch (error) {
      if (environmentId) await ipc.workspaceDeleteEnvironment(environmentId)
      await ipc.workspaceSaveGlobalVariables([])
      throw new Error('Round3 setup: ' + JSON.stringify(error))
    }
  }, base)
  let duplicateId
  try {
    assert.equal(fixture.environment.variables[0].value, '')
    assert.equal(fixture.environment.variables[0].hasValue, true)
    const relativePath = relative(root, fixture.path)
    assert.ok(relativePath && !relativePath.startsWith('..'), 'fixture workspace must remain under isolated E2E root')
    const stored = readFileSync(join(fixture.path, 'environments', fixture.environmentId + '.yaml'), 'utf8')
    assert.ok(!stored.includes('round3-synthetic-environment'))
    assert.ok(!readFileSync(join(fixture.path, 'globals.yaml'), 'utf8').includes('round3-synthetic-global'))

    await page.getByRole('button', { name: '环境配置', exact: true }).click()
    const modal = page.locator('.environment-modal')
    const secretInput = modal.locator('.variables-section').first().locator('.variable-value input').first()
    assert.equal(await secretInput.getAttribute('type'), 'password')
    assert.equal(await secretInput.inputValue(), '')
    assert.equal(await secretInput.getAttribute('placeholder'), '已设置；输入以替换')
    await modal.getByRole('button', { name: '完成', exact: true }).click()

    const count = received.length
    const previousResponseId = await page.evaluate(async () => (await import('/src/stores/tabs.ts')).useTabsStore().active?.response?.id)
    await page.getByRole('button', { name: '发送请求', exact: true }).click()
    const deadline = Date.now() + 10000
    while (received.length <= count && Date.now() < deadline) {
      await new Promise(resolve => setTimeout(resolve, 25))
    }
    await page.waitForFunction(async previous => {
      const tabs = (await import('/src/stores/tabs.ts')).useTabsStore()
      return Boolean((tabs.active?.response && tabs.active.response.id !== previous) || tabs.active?.error)
    }, previousResponseId)
    const sendError = await page.evaluate(async () => (await import('/src/stores/tabs.ts')).useTabsStore().active?.error)
    assert.equal(sendError, null, 'Round3 send failed: ' + JSON.stringify(sendError))
    const sendState = await page.evaluate(async () => {
      const tabs = (await import('/src/stores/tabs.ts')).useTabsStore()
      return { url: tabs.active?.url, nodeId: tabs.active?.nodeId, response: tabs.active?.response, error: tabs.active?.error }
    })
    assert.equal(received.length, count + 1, 'Round3 receiver mismatch: ' + JSON.stringify(sendState))
    const sent = received.at(-1)
    assert.equal(sent.url, '/round3/round3-synthetic-environment?trace=round3-synthetic-environment')
    assert.equal(sent.headers['x-correlation'], 'round3-synthetic-environment')
    assert.equal(sent.headers['x-global-private'], 'round3-synthetic-global')
    for (const scope of ['global', 'collection', 'folder']) assert.equal(sent.headers['x-' + scope], scope)
    assert.equal(sent.headers['x-shared'], 'environment')
    const publicState = await page.evaluate(async () => {
      const { ipc } = await import('/src/ipc/index.ts')
      const { useTabsStore } = await import('/src/stores/tabs.ts')
      return { snapshot: await ipc.workspaceLoad(), response: useTabsStore().active.response }
    })
    const publicJson = JSON.stringify(publicState)
    assert.ok(!publicJson.includes('round3-synthetic-environment'))
    assert.ok(!publicJson.includes('round3-synthetic-global'))
    assert.ok(publicState.response.actualRequest.url.includes('[redacted]'))

    const duplicate = await page.evaluate(async id => {
      const { ipc } = await import('/src/ipc/index.ts')
      return ipc.workspaceDuplicateEnvironment(id)
    }, fixture.environmentId)
    duplicateId = duplicate.id
    assert.equal(duplicate.variables[0].hasValue, true)
    assert.equal(duplicate.variables[0].value, '')
    assert.notEqual(duplicate.variables[0].id, fixture.environment.variables[0].id)
    // History contains only public projections, including the actual request URL/Header.
    const historyPath = join(root, 'runtime', fixture.workspaceId, 'history', publicState.response.id + '.yaml')
    const historyDeadline = Date.now() + 5000
    while (!existsSync(historyPath) && Date.now() < historyDeadline) {
      await new Promise(resolve => setTimeout(resolve, 25))
    }
    const history = readFileSync(historyPath, 'utf8')
    assert.ok(!history.includes('round3-synthetic-environment'))
    assert.ok(!history.includes('round3-synthetic-global'))
    console.log('E2E Round3: Windows credential store, masked IPC/UI/YAML/history, scoped HTTP send and environment copy passed')
  } finally {
    await page.evaluate(async ({ environmentId, duplicateId }) => {
      const { ipc } = await import('/src/ipc/index.ts')
      const { useWorkspaceStore } = await import('/src/stores/workspace.ts')
      if (duplicateId) await ipc.workspaceDeleteEnvironment(duplicateId)
      await ipc.workspaceDeleteEnvironment(environmentId)
      await ipc.workspaceSaveGlobalVariables([])
      const workspace = useWorkspaceStore()
      await workspace.load()
      workspace.selectEnvironment(null)
    }, { environmentId: fixture.environmentId, duplicateId })
  }
}
