import { useCallback, useRef } from 'react'

export type SplitDirection = 'horizontal' | 'vertical'

export interface SplitPane<P> {
  id: string
  /** Flex weight; normalized against siblings. */
  ratio: number
  payload: P
}

interface Props<P> {
  direction: SplitDirection
  panes: SplitPane<P>[]
  onRatioChange: (id: string, ratio: number) => void
  renderPane: (pane: SplitPane<P>) => React.ReactNode
}

/**
 * Single-level split container: `horizontal` lays panes side by side, `vertical` stacks
 * them. Dragging a handle only changes the two adjacent weights, which is enough for a
 * predictable layout and keeps persistence trivial (one ratio per pane).
 */
export function SplitView<P>({ direction, panes, onRatioChange, renderPane }: Props<P>) {
  const container = useRef<HTMLDivElement>(null)
  const horizontal = direction === 'horizontal'

  const startDrag = useCallback(
    (index: number, event: React.PointerEvent) => {
      event.preventDefault()
      const element = container.current
      if (!element) return
      const rect = element.getBoundingClientRect()
      const total = horizontal ? rect.width : rect.height
      const left = panes[index]
      const right = panes[index + 1]
      if (!left || !right) return
      const combined = left.ratio + right.ratio

      const move = (pointer: PointerEvent): void => {
        const offset = horizontal ? pointer.clientX - rect.left : pointer.clientY - rect.top
        const ratio = Math.min(Math.max(offset / Math.max(total, 1), 0.15), 0.85)
        onRatioChange(left.id, combined * ratio)
        onRatioChange(right.id, combined * (1 - ratio))
      }
      const stop = (): void => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', stop)
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', stop)
    },
    [horizontal, onRatioChange, panes]
  )

  return (
    <div ref={container} className={`flex h-full w-full ${horizontal ? 'flex-row' : 'flex-col'}`}>
      {panes.map((pane, index) => (
        <div key={pane.id} className="flex min-h-0 min-w-0" style={{ flex: `${pane.ratio} 1 0%` }}>
          <div className="min-h-0 min-w-0 flex-1">{renderPane(pane)}</div>
          {index < panes.length - 1 ? (
            <div
              onPointerDown={(event) => startDrag(index, event)}
              className={`shrink-0 bg-line transition hover:bg-accent ${
                horizontal ? 'w-[3px] cursor-col-resize' : 'h-[3px] cursor-row-resize'
              }`}
            />
          ) : null}
        </div>
      ))}
    </div>
  )
}
