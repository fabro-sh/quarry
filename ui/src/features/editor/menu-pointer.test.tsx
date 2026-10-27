import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import * as Menu from '@radix-ui/react-dropdown-menu';

function ExampleMenu() {
  return <Menu.Root defaultOpen modal={false}>
    <Menu.Trigger>Actions</Menu.Trigger>
    <Menu.Content>
      <Menu.Sub>
        <Menu.SubTrigger>Turn into</Menu.SubTrigger>
        <Menu.SubContent><Menu.Item>Heading</Menu.Item></Menu.SubContent>
      </Menu.Sub>
    </Menu.Content>
  </Menu.Root>;
}
function pointer(target: Element, type: string, pointerType: string) {
  const event = new Event(type, { bubbles: true });
  Object.defineProperties(event, { pointerType: { value: pointerType }, relatedTarget: { value: document.body } });
  fireEvent(target, event);
}
beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

for (const pointerType of ['', 'mouse', 'touch', 'pen']) {
  const mouse = pointerType === '' || pointerType === 'mouse';
  test(`${pointerType || 'unspecified'} pointer ${mouse ? 'opens' : 'does not open'} hover submenu`, () => {
    render(<ExampleMenu />);
    pointer(screen.getByText('Turn into'), 'pointermove', pointerType);
    act(() => { vi.advanceTimersByTime(150); });
    expect(screen.queryByText('Heading') !== null).toBe(mouse);
  });
  test(`${pointerType || 'unspecified'} pointer leave ${mouse ? 'cancels' : 'does not cancel'} a mouse hover`, () => {
    render(<ExampleMenu />);
    const trigger = screen.getByText('Turn into');
    pointer(trigger, 'pointermove', 'mouse');
    pointer(trigger, 'pointerout', pointerType);
    act(() => { vi.advanceTimersByTime(150); });
    expect(screen.queryByText('Heading') !== null).toBe(!mouse);
  });
}
