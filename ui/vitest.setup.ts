import '@testing-library/jest-dom/vitest';

if (typeof globalThis.localStorage?.getItem !== 'function') {
  const memory = new Map<string, string>();
  const storage: Storage = {
    get length() {
      return memory.size;
    },
    clear: () => memory.clear(),
    getItem: (key) => memory.get(key) ?? null,
    key: (index) => Array.from(memory.keys())[index] ?? null,
    removeItem: (key) => {
      memory.delete(key);
    },
    setItem: (key, value) => {
      memory.set(key, String(value));
    },
  };
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: storage,
  });
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    value: storage,
  });
}

if (typeof globalThis.ResizeObserver === 'undefined') {
  class TestResizeObserver implements ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  }

  Object.defineProperty(globalThis, 'ResizeObserver', {
    configurable: true,
    value: TestResizeObserver,
  });
  Object.defineProperty(window, 'ResizeObserver', {
    configurable: true,
    value: TestResizeObserver,
  });
}

if (typeof globalThis.IntersectionObserver === 'undefined') {
  // The TOC sidebar's scroll-spy constructs an IntersectionObserver on mount;
  // jsdom doesn't provide one. A no-op observer is enough — tests don't drive
  // real scrolling, and active-heading tracking is exercised in the browser.
  class TestIntersectionObserver implements IntersectionObserver {
    readonly root = null;
    readonly rootMargin = '';
    readonly thresholds = [];
    observe() {}
    unobserve() {}
    disconnect() {}
    takeRecords(): IntersectionObserverEntry[] {
      return [];
    }
  }

  Object.defineProperty(globalThis, 'IntersectionObserver', {
    configurable: true,
    value: TestIntersectionObserver,
  });
  Object.defineProperty(window, 'IntersectionObserver', {
    configurable: true,
    value: TestIntersectionObserver,
  });
}

if (typeof Element !== 'undefined' && typeof Element.prototype.scrollIntoView !== 'function') {
  Object.defineProperty(Element.prototype, 'scrollIntoView', {
    configurable: true,
    value: () => {},
  });
}
