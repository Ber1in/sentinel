import { contextBridge, ipcRenderer } from 'electron'

const api = {
  ping: (): Promise<{ pong: boolean; version: string }> => ipcRenderer.invoke('app:ping')
}

export type DesktopApi = typeof api

contextBridge.exposeInMainWorld('desktop', api)
