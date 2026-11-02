import { useEffect, useState } from 'react'

export function App() {
  const [version, setVersion] = useState<string>('…')

  useEffect(() => {
    void window.desktop.ping().then((result) => setVersion(result.version))
  }, [])

  return (
    <main style={{ fontFamily: 'system-ui, sans-serif', color: '#d7dae0', padding: 24 }}>
      <h1 style={{ fontSize: 20 }}>sshTerminal</h1>
      <p>桌面骨架已启动（app version {version}）。</p>
    </main>
  )
}

declare global {
  interface Window {
    desktop: import('../preload/index').DesktopApi
  }
}
