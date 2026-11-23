/**
 * Stand-in for the @reel/ui barrel, which pulls in Solid components. Bun shares `mock.module` across
 * test files in a run, so every file must register this same mock or results depend on file order.
 */
export const uiMock = () => ({
  getServerUrl: () => "https://reel.test",
  serverUrl: (p: string) => `http://reel.test${p}`,
});
