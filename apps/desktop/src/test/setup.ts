// Registers @testing-library/jest-dom matchers (toBeInTheDocument, toHaveClass, …) with Vitest's expect.
import "@testing-library/jest-dom/vitest";

// Node 22+ ships an experimental built-in `localStorage` global. Launched without a valid
// `--localstorage-file` it is a non-functional stub (no `.clear`/`.getItem`) that shadows the
// DOM-standard Storage jsdom installs on `window`. Component tests that touch localStorage then
// blow up with "localStorage.clear is not a function". When we detect that broken stub, swap in a
// minimal spec-compliant in-memory Storage so jsdom tests behave like a real browser. Guarded so
// pure-node tests (which never touch storage) are untouched.
function installInMemoryStorage(): void {
  const store = new Map<string, string>();
  const storage: Storage = {
    get length() {
      return store.size;
    },
    clear() {
      store.clear();
    },
    getItem(key: string) {
      return store.has(key) ? (store.get(key) as string) : null;
    },
    key(index: number) {
      return Array.from(store.keys())[index] ?? null;
    },
    removeItem(key: string) {
      store.delete(key);
    },
    setItem(key: string, value: string) {
      store.set(key, String(value));
    },
  };
  Object.defineProperty(globalThis, "localStorage", {
    value: storage,
    configurable: true,
    writable: true,
  });
}

if (typeof globalThis.localStorage === "undefined" || typeof globalThis.localStorage.clear !== "function") {
  installInMemoryStorage();
}
