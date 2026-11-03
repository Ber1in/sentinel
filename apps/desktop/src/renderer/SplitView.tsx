import { useCallback, useRef } from 'react'
import type { LayoutNode } from './layout'

interface Props {
  node: LayoutNode
  onRatioChange: (splitId: string, ratio: number) => void
  renderPane: (paneId: string) => React.ReactNode
}

/**
 * Nested split renderer. Each branch is a flex container with a draggable separator, so
 * horizontal and vertical splits can coexist at any depth.
 */
export function SplitView({ node, onRatioChange, renderPane }: Props) {
  if (node.kind === 'pane') {
    return <div className="h-full w-full min-h-0 min-w-0">{renderPane(node.id)}</div>
  }
  return (
    <SplitBranch
      node={node}
      onRatioChange={onRatioChange}
      renderPane={renderPane}
    />
  )
}

function SplitBranch({
  node,
  onRatioChange,
  renderPane
}: Props & { node: Extract<LayoutNode, { kind: 'split' }> }) {
  const container = useRef<HTMLDivElement>(null)
  const horizontal = node.direction === 'horizontal'

  const startDrag = useCallback(
    (event: React.PointerEvent) => {
      event.preventDefault()
      const element = container.current
      if (!element) return
      const rect = element.getBoundingClientRect()
      const total = horizontal ? rect.width : rect.height

      const move = (pointer: PointerEvent): void => {
        const offset = horizontal ? pointer.clientX - rect.left : pointer.clientY - rect.top
        const ratio = Math.min(Math.max(offset / Math.max(total, 1), 0.12), 0.88)
        onRatioChange(node.id, ratio)
      }
      const stop = (): void => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', stop)
      }
      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', stop)
    },
    [horizontal, node.id, onRatioChange]
  )

  return (
    <div
      ref={container}
      className={`flex h-full w-full min-h-0 min-w-0 ${horizontal ? 'flex-row' : 'flex-col'}`}
    >
      <div className="flex min-h-0 min-w-0" style={{ flex: `${node.ratio} 1 0%` }}>
        <SplitView node={node.first} onRatioChange={onRatioChange} renderPane={renderPane} />
      </div>

      {/* Separator: thick enough to see, with a grip so it reads as draggable. */}
      <div
        onPointerDown={startDrag}
        className={`group/sep relative flex shrink-0 items-center justify-center border-line bg-surface transition hover:bg-accent/60 ${
          horizontal ? 'w-[6px] cursor-col-resize border-x' : 'h-[6px] cursor-row-resize border-y'
        }`}
      >
        <span
          className={`rounded-full bg-line transition group-hover/sep:bg-accent ${
            horizontal ? 'h-6 w-[2px]' : 'h-[2px] w-6'
          }`}
        />
      </div>

      <div className="flex min-h-0 min-w-0" style={{ flex: `${1 - node.ratio} 1 0%` }}>
        <SplitView node={node.second} onRatioChange={onRatioChange} renderPane={renderPane} />
      </div>
    </div>
  )
}
