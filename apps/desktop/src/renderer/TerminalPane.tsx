import { useEffect, useRef } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebglAddon } from '@xterm/addon-webgl'

interface Props {
  sessionId: string
  numeric: number
  active: boolean
  onRegister: (numeric: number, terminal: Terminal | null) => void
}

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

export function TerminalPane({ sessionId, numeric, active, onRegister }: Props) {
  const container = useRef<HTMLDivElement>(null)
  const terminal = useRef<Terminal | null>(null)
  const fit = useRef<FitAddon | null>(null)

  useEffect(() => {
    const element = container.current
    if (!element) return

    const term = new Terminal({
      fontFamily: 'JetBrains Mono, monospace',
      fontSize: 13,
      lineHeight: 1.35,
      cursorBlink: true,
      allowProposedApi: true,
      scrollback: 100_000,
      theme: {
        background: cssVar('--canvas'),
        foreground: cssVar('--ink'),
        cursor: cssVar('--accent'),
        selectionBackground: cssVar('--line'),
        black: '#16181d',
        red: cssVar('--danger'),
        green: cssVar('--success'),
        yellow: cssVar('--warning'),
        blue: cssVar('--accent'),
        magenta: '#c297ff',
        cyan: '#39c5cf',
        white: '#e6e8ee'
      }
    })

    const fitAddon = new FitAddon()
    term.loadAddon(fitAddon)
    term.loadAddon(new Unicode11Addon())
    term.unicode.activeVersion = '11'
    term.open(element)
    try {
      term.loadAddon(new WebglAddon())
    } catch {
      // Canvas fallback is fine.
    }

    const sync = (): void => {
      try {
        fitAddon.fit()
        void window.desktop.resize(sessionId, term.cols, term.rows)
      } catch {
        // Layout not ready yet.
      }
    }

    term.onData((data) => {
      void window.desktop.sendInput(sessionId, new TextEncoder().encode(data))
    })

    const observer = new ResizeObserver(() => sync())
    observer.observe(element)
    requestAnimationFrame(sync)

    terminal.current = term
    fit.current = fitAddon
    onRegister(numeric, term)

    return () => {
      observer.disconnect()
      onRegister(numeric, null)
      term.dispose()
      terminal.current = null
    }
  }, [numeric, sessionId, onRegister])

  useEffect(() => {
    if (active && fit.current) {
      requestAnimationFrame(() => {
        try {
          fit.current?.fit()
          terminal.current?.focus()
        } catch {
          // ignore
        }
      })
    }
  }, [active])

  return (
    <div
      ref={container}
      className="h-full w-full px-3 py-2"
      style={{ display: active ? 'block' : 'none' }}
    />
  )
}
