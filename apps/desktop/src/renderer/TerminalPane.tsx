import { useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { Unicode11Addon } from '@xterm/addon-unicode11'
import { WebglAddon } from '@xterm/addon-webgl'
import { SearchAddon } from '@xterm/addon-search'
import { WebLinksAddon } from '@xterm/addon-web-links'

/**
 * Defensive: a session must own exactly one terminal. If a duplicate pane is ever
 * mounted, the second one stays inert instead of sending every keystroke twice.
 */
const mounted = new Set<string>()

interface Props {
  sessionId: string
  numeric: number
  active: boolean
  onRegister: (numeric: number, terminal: Terminal | null) => void
  onReady?: () => void
}

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

export function TerminalPane({ sessionId, numeric, active, onRegister, onReady }: Props) {
  const container = useRef<HTMLDivElement>(null)
  const terminal = useRef<Terminal | null>(null)
  const fit = useRef<FitAddon | null>(null)
  const search = useRef<SearchAddon | null>(null)
  const [searchOpen, setSearchOpen] = useState(false)
  const [query, setQuery] = useState('')

  useEffect(() => {
    const element = container.current
    if (!element) return
    if (mounted.has(sessionId)) {
      console.warn(`duplicate terminal for session ${sessionId} ignored`)
      return
    }
    mounted.add(sessionId)

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
    const searchAddon = new SearchAddon()
    term.loadAddon(searchAddon)
    search.current = searchAddon
    term.loadAddon(
      new WebLinksAddon((_event, uri) => {
        window.open(uri, '_blank')
      })
    )
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
      // xterm answers terminal capability queries (DA1/DA2). If those answers arrive
      // before tmux has taken over the tty they get echoed by the shell as
      // `0;276;0c…` junk. tmux falls back to sane defaults without them.
      const cleaned = data.replace(/\x1b\[[?>][0-9;]*c/g, '')
      if (!cleaned) return
      void window.desktop.sendInput(sessionId, new TextEncoder().encode(cleaned))
    })

    // Windows Terminal style copy/paste. xterm already wraps pasted text in bracketed
    // paste sequences when the remote application enabled DECSET 2004.
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown') return true
      if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'c') {
        const selection = term.getSelection()
        if (selection) void navigator.clipboard.writeText(selection)
        return false
      }
      if (event.ctrlKey && !event.shiftKey && event.key.toLowerCase() === 'f') {
        setSearchOpen(true)
        return false
      }
      if (event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'v') {
        void navigator.clipboard.readText().then((text) => {
          if (text) term.paste(text)
        })
        return false
      }
      return true
    })

    const observer = new ResizeObserver(() => sync())
    observer.observe(element)
    requestAnimationFrame(sync)

    terminal.current = term
    fit.current = fitAddon
    onRegister(numeric, term)
    onReady?.()

    return () => {
      observer.disconnect()
      onRegister(numeric, null)
      term.dispose()
      terminal.current = null
      mounted.delete(sessionId)
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
    <div className="relative h-full w-full" style={{ display: active ? 'block' : 'none' }}>
      {searchOpen ? (
        <div className="absolute right-3 top-2 z-10 flex items-center gap-1 rounded-lg border border-line bg-elevated px-2 py-1 shadow-lg">
          <input
            autoFocus
            value={query}
            onChange={(event) => {
              setQuery(event.target.value)
              search.current?.findNext(event.target.value, { incremental: true })
            }}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.shiftKey
                  ? search.current?.findPrevious(query)
                  : search.current?.findNext(query)
              }
              if (event.key === 'Escape') setSearchOpen(false)
            }}
            placeholder="搜索"
            className="w-40 bg-transparent text-[12px] outline-none"
          />
          <button
            className="rounded px-1 text-[11px] text-muted hover:text-ink"
            onClick={() => search.current?.findPrevious(query)}
          >
            ↑
          </button>
          <button
            className="rounded px-1 text-[11px] text-muted hover:text-ink"
            onClick={() => search.current?.findNext(query)}
          >
            ↓
          </button>
          <button
            className="rounded px-1 text-[11px] text-muted hover:text-ink"
            onClick={() => setSearchOpen(false)}
          >
            ✕
          </button>
        </div>
      ) : null}
      <div ref={container} className="h-full w-full px-3 py-2" />
    </div>
  )
}
