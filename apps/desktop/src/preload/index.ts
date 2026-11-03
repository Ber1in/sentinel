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

export interface SessionStateData {
  sessionId: string
  state: string
  detail: string | null
}

export interface TerminalData {
  numeric: number
  seq: number
  bytes: Uint8Array
}

const api = {
  daemonStatus: (): Promise<DaemonStatus> => ipcRenderer.invoke('daemon:status'),
  applyTitleBarTheme: (theme: 'dark' | 'light'): Promise<void> =>
    ipcRenderer.invoke('theme:titlebar', theme),
  restartDaemon: (): Promise<{ ok: boolean; error: string | null }> =>
    ipcRenderer.invoke('daemon:restart'),
  listHosts: (): Promise<HostSummary[]> => ipcRenderer.invoke('hosts:list'),
  listSessions: (): Promise<SessionSummary[]> => ipcRenderer.invoke('sessions:list'),
  createSession: (host: HostSpec, paneId: string, cwd: string | null): Promise<CreateSessionReply> =>
    ipcRenderer.invoke('session:create', host, paneId, cwd),
  setSecret: (id: string, secret: string): Promise<void> => ipcRenderer.invoke('secret:set', id, secret),
  deleteSecret: (id: string): Promise<void> => ipcRenderer.invoke('secret:delete', id),
  updateHost: (host: HostSpec, name: string, groupName: string | null): Promise<void> =>
    ipcRenderer.invoke('host:update', host, name, groupName),
  deleteHost: (hostId: string): Promise<void> => ipcRenderer.invoke('host:delete', hostId),
  saveLayout: (tabs: unknown[], panes: unknown[]): Promise<void> =>
    ipcRenderer.invoke('layout:save', tabs, panes),
  loadLayout: (): Promise<{ tabs: unknown[]; panes: unknown[] }> =>
    ipcRenderer.invoke('layout:load'),
  duplicateHost: (hostId: string, newId: string): Promise<void> =>
    ipcRenderer.invoke('host:duplicate', hostId, newId),
  sendInput: (sessionId: string, payload: Uint8Array): Promise<boolean> =>
    ipcRenderer.invoke('session:input', sessionId, payload),
  resync: (sessionId: string): Promise<void> => ipcRenderer.invoke('session:resync', sessionId),
  resize: (sessionId: string, cols: number, rows: number): Promise<void> =>
    ipcRenderer.invoke('session:resize', sessionId, cols, rows),
  detach: (sessionId: string): Promise<void> => ipcRenderer.invoke('session:detach', sessionId),
  destroy: (sessionId: string): Promise<void> => ipcRenderer.invoke('session:destroy', sessionId),
  onSessionState: (handler: (data: SessionStateData) => void): (() => void) => {
    const listener = (_event: unknown, data: SessionStateData): void => handler(data)
    ipcRenderer.on('session:state', listener)
    return () => ipcRenderer.removeListener('session:state', listener)
  },
  onTerminalData: (handler: (data: TerminalData) => void): (() => void) => {
    const listener = (_event: unknown, data: TerminalData): void => handler(data)
    ipcRenderer.on('terminal:data', listener)
    return () => ipcRenderer.removeListener('terminal:data', listener)
  }
}

export type DesktopApi = typeof api

contextBridge.exposeInMainWorld('desktop', api)
