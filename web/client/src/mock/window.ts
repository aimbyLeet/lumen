// Development only (`vite --mode mock`): the window's buttons do nothing in a browser.
export function getCurrentWindow() {
  const nothing = async () => {};
  return { minimize: nothing, toggleMaximize: nothing, close: nothing, isMaximized: async () => false };
}
