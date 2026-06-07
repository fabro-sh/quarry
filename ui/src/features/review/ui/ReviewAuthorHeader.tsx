import { AgentAvatar } from '../../agents/AgentAvatar';
import { agentKind } from '../../agents/agents';
import { firstWord, formatRelativeTime, initials } from '../format';

// Author byline shared by the comment and suggestion cards (rail + panel): a
// brand avatar, the author's first name over a relative timestamp, and an
// optional "Resolved" badge.
export function ReviewAuthorHeader({
  by,
  at,
  resolved,
}: {
  by: string;
  at: string;
  resolved?: boolean;
}) {
  return (
    <div className="flex items-center gap-2.5">
      <AgentAvatar
        className="bg-surface text-xs font-medium text-muted ring-1 ring-inset ring-line"
        fallback={initials(by)}
        kind={agentKind(by)}
      />
      <div className="flex min-w-0 flex-col">
        <span className="truncate text-sm font-medium leading-tight text-ink" title={by}>{firstWord(by)}</span>
        <span className="text-[11px] leading-tight text-faint">{formatRelativeTime(at)}</span>
      </div>
      {resolved ? <span className="ml-1 text-[11px] font-medium text-muted">Resolved</span> : null}
    </div>
  );
}
