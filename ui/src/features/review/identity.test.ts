import { beforeEach, describe, expect, it } from 'vitest';
import { currentAuthor, loadAuthor, normalizeAuthor, saveAuthor } from './identity';

describe('currentAuthor', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('defaults to "user"', () => {
    expect(currentAuthor()).toBe('user');
  });

  it('normalizes blank names to the default author', () => {
    expect(normalizeAuthor('  ')).toBe('user');
    expect(normalizeAuthor(null)).toBe('user');
  });

  it('loads and saves a self-provided author label', () => {
    expect(saveAuthor('  Avery  ')).toBe('Avery');
    expect(loadAuthor()).toBe('Avery');
    expect(currentAuthor()).toBe('Avery');
  });

  it('removes the stored author when reset to the default', () => {
    saveAuthor('Avery');
    saveAuthor('user');
    expect(localStorage.getItem('quarry:author')).toBeNull();
    expect(currentAuthor()).toBe('user');
  });
});
