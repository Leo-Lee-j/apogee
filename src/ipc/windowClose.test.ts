import { beforeEach, expect, it, vi } from 'vitest'
const native = vi.hoisted(() => ({ close: vi.fn(), listen: vi.fn() }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ close: native.close, onCloseRequested: native.listen }) }))

beforeEach(() => { vi.resetModules(); vi.resetAllMocks(); Object.assign(globalThis, { window: { __TAURI_INTERNALS__: { invoke: vi.fn() } } }) })
it('prevents native destruction, serializes close attempts, and allows one prepared close', async () => {
  const { windowControls } = await import('./index')
  let handler!: (event: { preventDefault: () => void }) => void
  native.listen.mockImplementationOnce(fn => { handler = fn; return () => {} })
  let finish!: (value: boolean) => void
  const prepare = vi.fn(() => new Promise<boolean>(resolve => { finish = resolve }))
  await windowControls.installCloseGuard(prepare, vi.fn())
  const preventDefault = vi.fn()
  handler({ preventDefault }); handler({ preventDefault })
  expect(preventDefault).toHaveBeenCalledTimes(2); expect(prepare).toHaveBeenCalledTimes(1)
  expect(native.close).not.toHaveBeenCalled()
  finish(false); await vi.waitFor(() => expect(prepare).toHaveBeenCalledTimes(1))
  await Promise.resolve()
  handler({ preventDefault }); finish(true)
  await vi.waitFor(() => expect(native.close).toHaveBeenCalledTimes(1))
  preventDefault.mockClear(); handler({ preventDefault })
  expect(preventDefault).not.toHaveBeenCalled()
})
