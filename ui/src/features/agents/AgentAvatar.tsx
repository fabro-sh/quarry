import type { ReactNode } from 'react';

import { cn } from '../../lib/utils';
import { agentIconSrc } from './agents';

// A circular avatar. When `kind` is a known agent provider it shows that
// provider's brand logo; otherwise it shows `fallback` (initials, a Bot icon,
// …). Size, ring, and background come from `className` so each caller blends
// with its surroundings.
export function AgentAvatar({
  kind,
  fallback,
  className,
}: {
  kind: string | null;
  fallback: ReactNode;
  className?: string;
}) {
  return (
    <span
      className={cn('flex size-7 shrink-0 items-center justify-center rounded-full', className)}
    >
      {kind ? <img alt="" className="size-full p-1" src={agentIconSrc(kind)} /> : fallback}
    </span>
  );
}
