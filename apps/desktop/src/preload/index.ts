import { contextBridge, ipcRenderer } from 'electron'

import type { HostSpec } from '../shared/HostSpec'
import type { HostSummary } from '../shared/HostSummary'
import type { ServerInfo } from '../shared/ServerInfo'
import type { SessionSummary } from '../shared/SessionSummary'

export interface DaemonStatus {
  connected: boolean
  info: ServerInfo | null
  port: number | null
  error: string | null
  mismatch: { daemonVersion: number; clientVersion: number } | null
}

export interface CreateSessionReply {
  session_id: string
  numeric: number
  provider: 'tmux' | 'none'
  recreated: boolean
}

export interface TerminalData {
  numeric: number
  seq: number
  bytes: Uint8Array
}

const api = {
  daemonStatus: (): Promise<DaemonStatus> => ipcRenderer.invoke('daemon:status'),
  listHosts: (): Promise<HostSummary[]> => ipcRenderer.invoke('hosts:list'),
  listSessions: (): Promise<SessionSummary[]> => ipcRenderer.invoke('sessions:list'),
  createSession: (host: HostSpec, paneId: string, cwd: string | null): Promise<CreateSessionReply> =>
    ipcRenderer.invoke('session:create', host, paneId, cwd),
  setSecret: (id: string, secret: string): Promise<void> => ipcRenderer.invoke('secret:set', id, secret),
  sendInput: (sessionId: string, payload: Uint8Array): Promise<boolean> =>
    ipcRenderer.invoke('session:input', sessionId, payload),
  resize: (sessionId: string, cols: number, rows: number): Promise<void> =>
    ipcRenderer.invoke('session:resize', sessionId, cols, rows),
  detach: (sessionId: string): Promise<void> => ipcRenderer.invoke('session:detach', sessionId),
  destroy: (sessionId: string): Promise<void> => ipcRenderer.invoke('session:destroy', sessionId),
  onTerminalData: (handler: (data: TerminalData) => void): (() => void) => {
    const listener = (_event: unknown, data: TerminalData): void => handler(data)
    ipcRenderer.on('terminal:data', listener)
    return () => ipcRenderer.removeListener('terminal:data', listener)
  }
}

export type DesktopApi = typeof api

contextBridge.exposeInMainWorld('desktop', api)
