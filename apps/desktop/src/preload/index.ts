import { contextBridge, ipcRenderer } from 'electron'

import type { ServerInfo } from '../shared/ServerInfo'
import type { SessionSummary } from '../shared/SessionSummary'

export interface DaemonStatus {
  connected: boolean
  info: ServerInfo | null
  port: number | null
  error: string | null
}

const api = {
  daemonStatus: (): Promise<DaemonStatus> => ipcRenderer.invoke('daemon:status'),
  listSessions: (): Promise<SessionSummary[]> => ipcRenderer.invoke('daemon:sessions'),
  ping: (): Promise<{ pong: boolean; daemon: ServerInfo; port: number }> =>
    ipcRenderer.invoke('daemon:ping')
}

export type DesktopApi = typeof api

contextBridge.exposeInMainWorld('desktop', api)
