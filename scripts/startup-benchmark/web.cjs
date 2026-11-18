/* Run with PLAYWRIGHT_MODULE pointing to a locally installed playwright package. */
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || "playwright");

async function main() {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROME_PATH ? { executablePath: process.env.CHROME_PATH } : {}),
    args: ["--autoplay-policy=no-user-gesture-required"],
  });
  try {
    const page = await browser.newPage();
    await page.addInitScript(() => localStorage.setItem("reel.token", "mock-token"));
    page.on("console", (message) => {
      if (message.text().startsWith("[ReelStartup] ")) console.log(message.text());
    });
    for (let attempt = 0; attempt < 20; attempt++) {
      await page.goto("http://127.0.0.1:15173/item/101");
      await page.getByRole("link", { name: "Resume", exact: false }).first().click();
      await page.waitForTimeout(2000);
    }
  } finally {
    await browser.close();
  }
}
main().catch((error) => { console.error(error.message); process.exitCode = 1; });
