import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { AgentAvatar } from './AgentAvatar';

describe('AgentAvatar', () => {
  it('renders the provider brand logo for a known agent kind', () => {
    const { container } = render(<AgentAvatar kind="claude" fallback="C" />);

    expect(container.querySelector('img')?.getAttribute('src')).toBe('/agent-icons/claude.svg');
    expect(screen.queryByText('C')).not.toBeInTheDocument();
  });

  it('renders the fallback when there is no agent kind', () => {
    const { container } = render(<AgentAvatar kind={null} fallback="C" />);

    expect(container.querySelector('img')).toBeNull();
    expect(screen.getByText('C')).toBeInTheDocument();
  });
});
