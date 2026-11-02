import { useCallback, useEffect, useState } from 'react'

import type { ServerInfo } from '../shared/ServerInfo'
import type { SessionSummary } from '../shared/SessionSummary'

interface Status {
  connected: boolean
  info: ServerInfo | null
  port: number | null
  error: string | null
}

export function App() {
  const [status, setStatus] = useState<Status>({ connected: false, info: null, port: null, error: null })
  const [sessions, setSessions] = useState<SessionSummary[]>([])

  const refresh = useCallback(async () => {
    const next = (await window.desktop.daemonStatus()) as Status
    setStatus(next)
    if (next.connected) {
      setSessions(await window.desktop.listSessions())
    } else {
      setSessions([])
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  return (
    <main style={{ fontFamily: 'system-ui, sans-serif', color: '#d7dae0', background: '#111318', minHeight: '100vh', padding: 24 }}>
      <h1 style={{ fontSize: 20, marginBottom: 4 }}>sshTerminal</h1>
      <p style={{ color: '#8b93a1', marginTop: 0 }}>守护进程与桌面壳已连通。</p>

      <section style={{ marginTop: 24 }}>
        <h2 style={{ fontSize: 14, color: '#8b93a1' }}>守护进程</h2>
        {status.connected && status.info ? (
          <ul style={{ lineHeight: 1.8 }}>
            <li>pid: {status.info.pid}</li>
            <li>版本: {status.info.daemon_version}（协议 v{status.info.protocol_version}）</li>
            <li>端口: {status.port}</li>
            <li>会话数: {sessions.length}</li>
          </ul>
        ) : (
          <p style={{ color: '#e0b341' }}>未连接：{status.error ?? '守护进程未运行'}</p>
        )}
        <button onClick={() => void refresh()}>重新检测</button>
      </section>
    </main>
  )
}

declare global {
  interface Window {
    desktop: import('../preload/index').DesktopApi
  }
}
