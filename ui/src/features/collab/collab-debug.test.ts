import { collabDebugEnabledFrom } from './collab-debug';

describe('collabDebugEnabledFrom', () => {
  it('enables when the collabDebug query param is present (no value)', () => {
    expect(collabDebugEnabledFrom('?collabDebug', null)).toBe(true);
  });

  it('enables for truthy query values and disables for explicit off values', () => {
    expect(collabDebugEnabledFrom('?collabDebug=1', null)).toBe(true);
    expect(collabDebugEnabledFrom('?collabDebug=0', null)).toBe(false);
    expect(collabDebugEnabledFrom('?collabDebug=false', null)).toBe(false);
  });

  it('falls back to the stored flag when the query param is absent', () => {
    expect(collabDebugEnabledFrom('', '1')).toBe(true);
    expect(collabDebugEnabledFrom('', 'true')).toBe(true);
    expect(collabDebugEnabledFrom('', null)).toBe(false);
    expect(collabDebugEnabledFrom('', '0')).toBe(false);
  });

  it('lets an explicit query value override the stored flag', () => {
    expect(collabDebugEnabledFrom('?collabDebug=0', '1')).toBe(false);
  });
});
