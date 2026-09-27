import type { CSSProperties } from 'react'

/** One stack: 1–3 messages fill one layer, 4–6 two, and 7+ all three. */
export function WorkflowInputGateQueue({ count, color }: { count: number; color: string }) {
  const filledLayers = Math.min(3, Math.ceil(count / 3))
  return (
    <svg
      className="workflow-monitor__gate-queue"
      viewBox="0 0 64 56"
      aria-hidden="true"
      data-queue-count={count}
      data-overloaded={count > 9 || undefined}
      style={{ '--workflow-queue-color': count > 9 ? '#d97706' : color } as CSSProperties}
    >
      <text className="workflow-monitor__queue-count" x="18" y="12" textAnchor="middle">
        {count}
      </text>
      {[0, 1, 2].map((layer) => (
        <g
          key={layer}
          className="workflow-monitor__queue-layer"
          data-filled={layer < filledLayers}
          transform={`translate(18 ${34 - layer * 8})`}
        >
          <path d="M -9 0 V 5 C -9 9 9 9 9 5 V 0 Z" />
          <ellipse cx="0" cy="0" rx="9" ry="3" />
        </g>
      ))}
    </svg>
  )
}
