import { describe, expect, test } from 'vitest';

import { parseWorkspaceRoute } from './workspace-navigation';

describe('workspace route parsing', () => {
  test('parses library document identity from the URL', () => {
    expect(parseWorkspaceRoute('/lib/team%20notes/documents/folder/daily%20log.md')).toEqual({
      scope: 'library',
      library: 'team notes',
      path: 'folder/daily log.md',
      createTmp: false,
    });
  });

  test('parses tmp capability routes without treating the secret as an identity', () => {
    expect(parseWorkspaceRoute('/tmp/abc%20123')).toEqual({
      scope: 'tmp',
      library: null,
      path: 'abc 123',
      createTmp: false,
    });
  });

  test('keeps the tmp creation route distinct from an open document', () => {
    expect(parseWorkspaceRoute('/tmp/new')).toEqual({
      scope: 'tmp',
      library: null,
      path: '',
      createTmp: true,
    });
  });
});
