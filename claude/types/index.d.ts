/** One band notice (hooks/band.js): outcome, action, status, its ref ids, and either the
 * elapsed time or, while running, when it started. */
export type BandNotice = {
  outcome: 'done' | 'unchanged' | 'look' | 'failed' | 'running' | 'hint'
  what: string
  status: string
  refs?: { kind: string; id: string }[]
  elapsed?: string
  since?: number
}

/** One band line: its notice, when it came in epoch ms, and the key a later notice
 * replaces it by. */
export type BandLine = { at: number; notice: BandNotice; key?: string }

declare module 'claude-code' {
  interface PluginState {
    'jev-context-compaction': { band: BandLine[] }
    // The shared band's peer: read here, written by its owner. Agent OS's `drawsNotices`
    // is read too, but typed by Agent OS's own contract (declaring it here clashes).
    'jev-input-standardizer': { band: BandLine[] }
  }
}
