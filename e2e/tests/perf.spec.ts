import { test, expect } from "./fixtures";
import { createCardInOwnDeck } from "./helpers";

// Real-user monitoring: rum.js (inlined in every page's <head>) posts one
// anonymous report per page load to /api/perf; perf.rs stores it and
// /api/perf/summary reports percentiles per release and route.

test("a page load sends a report with timings, and the server stores it", async ({ page }) => {
  await page.goto("/");

  const stored = page.waitForResponse(
    (res) => res.url().endsWith("/api/perf") && res.request().method() === "POST",
  );
  const sent = page.waitForRequest(
    (req) => req.url().endsWith("/api/perf") && req.method() === "POST",
  );
  await page.evaluate(() => (window as unknown as { __rumFlush: () => void }).__rumFlush());

  const report = (await sent).postDataJSON();
  expect(report.path).toBe("/");
  expect(report.ttfb_ms).toBeGreaterThan(0);
  expect(report.fcp_ms).toBeGreaterThan(0);
  expect(report.hydrated_ms).toBeGreaterThan(0);
  expect(report.js_error).toBeUndefined();
  // The server reports its own timing on the navigation response.
  expect(report.server_ms).toBeGreaterThanOrEqual(0);
  expect(report.server_uptime_s).toBeGreaterThanOrEqual(0);
  expect((await stored).status()).toBe(204);
});

test("the summary reports percentiles per release and route, with ids collapsed", async ({
  page,
}) => {
  const { word } = await createCardInOwnDeck(page, "perf-spec");
  await page.goto("/");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  await expect(page).toHaveURL(/\/cards\/[0-9a-f-]+$/);

  const stored = page.waitForResponse(
    (res) => res.url().endsWith("/api/perf") && res.request().method() === "POST",
  );
  // A hard load of the card page, so a report for it exists.
  await page.reload();
  await page.locator('body[data-hydrated="true"]').waitFor();
  await page.evaluate(() => (window as unknown as { __rumFlush: () => void }).__rumFlush());
  await stored;

  const res = await page.request.get("/api/perf/summary?days=1");
  expect(res.status()).toBe(200);
  const rows = (await res.json()) as Array<{ path: string; loads: number; ttfb_p50: number }>;
  const card = rows.find((r) => r.path === "/cards/:id");
  expect(card, "a /cards/:id row").toBeTruthy();
  expect(card!.loads).toBeGreaterThan(0);
  expect(card!.ttfb_p50).toBeGreaterThan(0);
  // No raw UUID ever becomes a route.
  expect(rows.some((r) => /[0-9a-f]{8}-[0-9a-f]{4}-/.test(r.path))).toBe(false);
});

test("the summary needs a signed-in session", async ({ browser, baseURL }) => {
  const anon = await browser.newContext({ storageState: { cookies: [], origins: [] }, baseURL });
  try {
    const res = await anon.request.get("/api/perf/summary");
    expect(res.status()).toBe(401);
  } finally {
    await anon.close();
  }
});

test("the beacon endpoint rejects garbage and oversized bodies instead of storing them", async ({
  page,
}) => {
  const garbage = await page.request.post("/api/perf", {
    headers: { "content-type": "application/json" },
    data: "not json",
  });
  expect(garbage.status()).toBeGreaterThanOrEqual(400);
  expect(garbage.status()).toBeLessThan(500);

  const huge = await page.request.post("/api/perf", {
    headers: { "content-type": "application/json" },
    data: JSON.stringify({ path: "/", js_error: "x".repeat(50_000) }),
  });
  expect(huge.status()).toBe(413);
});
