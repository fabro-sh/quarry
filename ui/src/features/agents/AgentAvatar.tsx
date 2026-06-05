import type { ReactNode } from 'react';

import { cn } from '../../lib/utils';
import { agentBrand, agentIconSrc } from './agents';

// A circular avatar. When `kind` is a known agent provider it shows that
// provider's brand logo on a tinted background; otherwise it shows `fallback`
// (initials, a Bot icon, …). Size, ring, and the neutral background come from
// `className` so each caller blends with its surroundings.
export function AgentAvatar({
  kind,
  fallback,
  className,
}: {
  kind: string | null;
  fallback: ReactNode;
  className?: string;
}) {
  const brand = kind ? agentBrand(kind) : undefined;
  return (
    <span
      className={cn('flex size-7 shrink-0 items-center justify-center rounded-full', className)}
      style={undefined}
    >
      {kind && brand ? <img alt="" className="size-full p-1" src={agentIconSrc(kind)} /> : fallback}
    </span>
  );
}
