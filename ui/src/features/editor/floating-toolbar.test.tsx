import { act, cleanup, render } from '@testing-library/react';
import { renderToString } from 'react-dom/server';
import { useCommittedFloatingToolbar } from './floating-toolbar';

const fixture = vi.hoisted(() => ({ revision: 0, update: vi.fn(), events: [] as string[] }));
vi.mock('platejs/react', () => ({ useEditorVersion: () => fixture.revision }));
vi.mock('@platejs/floating', () => ({
  // Match Plate 52's selector: it eagerly asks for a position while rendering.
  useFloatingToolbar: ({ floating }: { floating: { update: () => void } }) => {
    floating.update();
    return { hidden: false };
  },
}));
afterEach(() => { cleanup(); fixture.update.mockReset(); fixture.events = []; fixture.revision = 0; });

function Toolbar() {
  const state = { floating: { update: fixture.update } } as unknown as Parameters<typeof useCommittedFloatingToolbar>[0];
  const toolbar = useCommittedFloatingToolbar(state);
  return <div data-hidden={toolbar.hidden} />;
}

test('an uncommitted render never measures the selection', () => {
  renderToString(<Toolbar />);
  expect(fixture.update).not.toHaveBeenCalled();
});

test('position measurement follows the DOM commit and each editor revision', () => {
  fixture.update.mockImplementation(() => fixture.events.push(document.querySelector('[data-hidden]') ? 'committed' : 'missing'));
  const view = render(<Toolbar />);
  expect(fixture.events).toEqual(['committed']);
  act(() => { fixture.revision++; view.rerender(<Toolbar />); });
  expect(fixture.events).toEqual(['committed', 'committed']);
  view.rerender(<Toolbar />);
  expect(fixture.events).toEqual(['committed', 'committed']);
});
