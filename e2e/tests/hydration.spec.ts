import { Page } from "@playwright/test";
import { test, expect } from "./fixtures";
import { uniqueWord } from "./helpers";

// A hydration panic leaves a page server-rendered but dead: no handlers
// run, so buttons (Record, Save, ...) silently do nothing. It was seen on
// production's card edit pages, with every control unresponsive. Two
// things catch it: the shared `page` fixture's goto already waits for
// body[data-hydrated] — which hydrate() only sets after hydrate_body
// returns, so a panic times out there — and the listener below, which
// fails on any uncaught error or console error while visiting the page.
function collectErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  page.on("console", (m) => {
    if (m.type() === "error" && !m.text().includes("Failed to load resource")) {
      errors.push(`console.error: ${m.text()}`);
    }
  });
  return errors;
}

async function createCard(page: Page, type: string): Promise<string> {
  const word = uniqueWord(`hydrate-${type}`);
  await page.goto("/new");
  await page.getByLabel("Card type").selectOption(type);
  if (type === "sentence_translation") {
    await page.getByLabel("Known-language sentence").fill("a known sentence");
  }
  await page.getByLabel(/^Target-language/).fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");
  const href = await page
    .locator("li", { hasText: word })
    .getByRole("link", { name: "Edit" })
    .getAttribute("href");
  if (!href) throw new Error(`no edit link for ${word}`);
  return href;
}

test("the main pages hydrate without errors", async ({ page }) => {
  const errors = collectErrors(page);
  for (const path of ["/", "/new", "/bulk-add", "/decks", "/dictionary", "/wiki", "/review"]) {
    await page.goto(path);
  }
  expect(errors).toEqual([]);
});

test("a card's edit page hydrates without errors, for every card type", async ({ page }) => {
  const types = [
    "image_to_target",
    "sound_to_written",
    "image_to_audio",
    "video_to_written",
    "sentence_translation",
  ];
  const hrefs: string[] = [];
  for (const type of types) {
    hrefs.push(await createCard(page, type));
  }

  const errors = collectErrors(page);
  for (const href of hrefs) {
    await page.goto(href);
    // Hydrated *and* alive: a control actually responds.
    await expect(page.getByLabel("Card type")).toBeEnabled();
  }
  expect(errors).toEqual([]);
});

test("a dictionary entry page hydrates without errors", async ({ page }) => {
  await createCard(page, "image_to_target");
  const errors = collectErrors(page);
  await page.goto("/dictionary");
  await page.locator(".dictionary-list li a").first().click();
  await expect(page).toHaveURL(/\/dictionary\/[0-9a-f-]+$/);
  await page.reload();
  await page.locator('body[data-hydrated="true"]').waitFor();
  expect(errors).toEqual([]);
});
