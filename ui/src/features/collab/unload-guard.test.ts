import { describe, expect, it } from 'vitest';

import { registerUnloadGuard } from './unload-guard';

function fireBeforeUnload(target: Window): Event {
  const event = new Event('beforeunload', { cancelable: true });
  target.dispatchEvent(event);
  return event;
}

describe('registerUnloadGuard', () => {
  it('prompts when closing while edits are not yet durable', () => {
    const unregister = registerUnloadGuard(window, () => true);
    const event = fireBeforeUnload(window);
    expect(event.defaultPrevented).toBe(true);
    unregister();
  });

  it('stays silent when everything is durable', () => {
    const unregister = registerUnloadGuard(window, () => false);
    const event = fireBeforeUnload(window);
    expect(event.defaultPrevented).toBe(false);
    unregister();
  });

  it('stops prompting after unregistering', () => {
    const unregister = registerUnloadGuard(window, () => true);
    unregister();
    const event = fireBeforeUnload(window);
    expect(event.defaultPrevented).toBe(false);
  });
});
