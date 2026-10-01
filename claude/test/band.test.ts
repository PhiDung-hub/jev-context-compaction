// `claude plugin test packs/jev-context-compaction/claude`: the module under Claude
// Code's own engine, the local compactor and the built-in compaction answered here.
import { expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

const BAND = {
  component: 'AbovePrompt',
  props: { hasSurvey: false, isWorking: false, maxRows: 8, bodyColumns: 120, scroll: { offset: 0, bodyRows: 7 }, view: {} },
} as const
const MESSAGES = [{ role: 'user', text: 'fix it', toolUses: [] }]
const STATS = { charsBefore: 100, charsAfter: 26, calls: 1, requests: 1, elapsedMs: 1200 }

// The compactor answers after 1.2 s of the mocked clock; `charsAfter` sets the reduction.
function world(on: On, answer: { status: number; body: unknown }) {
  const clock = mock.clock(on, { now: 1_790_000_000_000 })
  const toasts: string[] = []
  on('ui.toast', (_$, e) => (toasts.push(e.text), { value: undefined }))
  on('ui.log', () => ({ value: undefined }))
  on('ui.render', { component: 'AbovePrompt' }, () => ({ type: 'Box', children: [] }))
  on('session.compact', (_$, e) => ({ messages: e.messages }))
  on('http.fetch', async () => {
    await clock.sleep(1_200)
    const text = JSON.stringify(answer.body)
    return { value: { status: answer.status, ok: answer.status === 200, headers: {}, text } }
  })
  return { clock, toasts }
}

async function compact($: Parameters<Parameters<typeof test>[1]>[0], clock: { advance: (ms: number) => Promise<void> }, agentId?: string) {
  // The engine hands the hook the transcript.
  const done = $.session.compact({ trigger: 'manual', messages: MESSAGES, ...(agentId && { agentId }) } as never)
  await clock.advance(1_200)
  return done
}

test('where no band draws, each compaction toasts its diamond, outcome and time', async ($, on) => {
  const { clock, toasts } = world(on, { status: 200, body: { messages: MESSAGES, stats: { ...STATS, charsAfter: 83 } } })
  await compact($, clock)
  expect(toasts).toEqual(['◊ compact · fallback: reduction 17% below 25% · 1.2s'])
})

test('a failed compactor call toasts as a fallback', async ($, on) => {
  const { clock, toasts } = world(on, { status: 413, body: { message: 'secret' } })
  await compact($, clock)
  expect(toasts).toEqual(['◊ compact · fallback: HTTP 413 · 1.2s'])
})

test('the band draws the notice in colour instead of a toast, then lets it go', async ($, on) => {
  const { clock, toasts } = world(on, { status: 200, body: { messages: MESSAGES, stats: STATS } })
  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ plugin: 'jev-context-compaction', surface, ...BAND })
    await compact($, clock)
    expect(toasts).toEqual([])
    const props = async (text: string) => (await ui.find({ type: 'Text', text }))?.props
    expect(await props('◆ ')).toEqual({ color: '#9ece6a' })
    expect(await props('compact')).toEqual({ color: '#7dcfff' })
    expect(await props('applied 74%')).toEqual({ color: '#9ece6a' })
    expect(await props('1.2s')).toEqual({ color: '#bb9af7' })
    await clock.advance(15_100)
    expect(await ui.find({ type: 'Text', text: '◆ ' })).toBeUndefined()
    await ui.unmount()
  }
})

test("the Jev request id and a subagent's id show blue and underlined", async ($, on) => {
  const { clock, toasts } = world(on, { status: 200, body: { messages: MESSAGES, stats: { ...STATS, requestIds: ['req_7f3a'] } } })
  const ui = await $.ui.mount({ plugin: 'jev-context-compaction', surface: 'terminal', ...BAND })
  await compact($, clock, 'a9f3c2')
  expect(toasts).toEqual([])
  for (const id of ['req_7f3a', 'a9f3c2']) expect((await ui.find({ type: 'Text', text: id }))?.props).toEqual({ color: '#7aa2f7', underline: true })
  await ui.unmount()
})

test('the band yields to a survey: the notice toasts', async ($, on) => {
  const { clock, toasts } = world(on, { status: 200, body: { messages: MESSAGES, stats: STATS } })
  const ui = await $.ui.mount({ plugin: 'jev-context-compaction', surface: 'terminal', ...BAND, props: { ...BAND.props, hasSurvey: true } })
  await compact($, clock)
  expect(toasts).toEqual(['◆ compact · applied 74% · 1.2s'])
  await ui.unmount()
})

test('while the compactor works, a running line counts up, then the outcome replaces it', async ($, on) => {
  const { clock, toasts } = world(on, { status: 200, body: { messages: MESSAGES, stats: STATS } })
  const ui = await $.ui.mount({ plugin: 'jev-context-compaction', surface: 'terminal', ...BAND })
  const done = $.session.compact({ trigger: 'manual', messages: MESSAGES } as never)
  await clock.advance(800)
  expect((await ui.find({ type: 'Text', text: '◌ ' }))?.props).toEqual({ color: '#e0af68' })
  expect((await ui.find({ type: 'Text', text: '0.8s' }))?.props).toEqual({ color: '#bb9af7' })
  await clock.advance(400)
  await done
  expect(await ui.find({ type: 'Text', text: '◌ ' })).toBeUndefined()
  expect(await ui.find({ type: 'Text', text: 'applied 74%' })).toBeDefined()
  expect(toasts).toEqual([])
  await ui.unmount()
})
