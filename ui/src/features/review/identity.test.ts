import { beforeEach, describe, expect, it } from 'vitest';
import {
  currentAuthor,
  DEFAULT_AUTHOR,
  hasStoredAuthor,
  loadAuthor,
  normalizeAuthor,
  saveAuthor,
  storedAuthor,
} from './identity';

describe('currentAuthor', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('defaults to "user"', () => {
    expect(currentAuthor()).toBe('user');
    expect(DEFAULT_AUTHOR).toBe('user');
  });

  it('reports whether an author was explicitly stored', () => {
    expect(hasStoredAuthor()).toBe(false);
    saveAuthor('Avery');
    expect(hasStoredAuthor()).toBe(true);
    saveAuthor('user');
    expect(hasStoredAuthor()).toBe(false);
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

  it('exposes the explicitly chosen author and omits the default', () => {
    expect(storedAuthor()).toBeUndefined();
    saveAuthor('Avery');
    expect(storedAuthor()).toBe('Avery');
    saveAuthor('user');
    expect(storedAuthor()).toBeUndefined();
  });

  it('removes the stored author when reset to the default', () => {
    saveAuthor('Avery');
    saveAuthor('user');
    expect(localStorage.getItem('quarry:author')).toBeNull();
    expect(currentAuthor()).toBe('user');
  });
});
