import { Page, Response } from "@playwright/test";
import { test, expect } from "./fixtures";
import {
  createCardInOwnDeck,
  forgetThumbnail,
  makePng,
  naturalWidth,
  recordedThumbnail,
  uniqueWord,
  withFormResponse,
  withServerResponse,
} from "./helpers";

// The card grid and the review screen show a small preview of each image;
// only a card's own page loads the full-size original. See thumbnail.rs.
const BIG = { width: 1200, height: 800 };

// Remembers every response the page receives, by URL, so a test can inspect
// the size and headers of the images the browser actually loaded (rather than
// re-requesting them from the test runner, which can't resolve the
// *.localhost storage host the way the browser does).
function recordResponses(page: Page): Map<string, Response> {
  const responses = new Map<string, Response>();
  page.on("response", (res) => responses.set(res.url(), res));
  return responses;
}

// Creates a card with `png` uploaded as its image, returns its word.
async function createCardWithImage(page: Page, png: Buffer, prefix: string): Promise<string> {
  const word = uniqueWord(prefix);
  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.locator('input[name="image"]').setInputFiles({
    name: "card.png",
    mimeType: "image/png",
    buffer: png,
  });
  // The upload is decoded and thumbnailed before the redirect, so on the
  // preview stack it can outlast the default 5s: wait for the POST to be
  // answered, then check where it sent us.
  await withFormResponse(page, /^\/cards\/new$/, () =>
    page.getByRole("button", { name: "Add" }).click(),
  );
  await expect(page).toHaveURL("/");
  return word;
}

test("a big uploaded image shows as a small preview in the card grid and in full on the card page", async ({
  page,
}) => {
  const responses = recordResponses(page);
  const word = await createCardWithImage(page, makePng(BIG.width, BIG.height), "thumb-big");

  const gridImage = page.locator("li", { hasText: word }).locator("img");
  const thumbSrc = await gridImage.getAttribute("src");
  expect(thumbSrc).toBeTruthy();
  // A preview no wider than 640px, not the 1200px original.
  expect(await naturalWidth(gridImage)).toBeLessThanOrEqual(640);

  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  const fullImage = page.locator("img").first();
  const fullSrc = await fullImage.getAttribute("src");
  expect(fullSrc).toBeTruthy();
  expect(fullSrc).not.toBe(thumbSrc);
  expect(await naturalWidth(fullImage)).toBe(BIG.width);

  // Both the full image and its preview are recorded against each other,
  // and the preview the browser downloaded is a fraction of the size.
  expect(await recordedThumbnail(fullSrc!)).toBe(thumbSrc);
  const thumbBytes = (await responses.get(thumbSrc!)!.body()).length;
  const fullBytes = (await responses.get(fullSrc!)!.body()).length;
  expect(thumbBytes * 5).toBeLessThan(fullBytes);
});

test("an already-small image is used as it is, with no separate preview", async ({ page }) => {
  const word = await createCardWithImage(page, makePng(60, 40, false), "thumb-small");

  const gridSrc = await page
    .locator("li", { hasText: word })
    .locator("img")
    .getAttribute("src");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  const fullSrc = await page.locator("img").first().getAttribute("src");
  expect(gridSrc).toBe(fullSrc);
});

test("stored images are cached for a year, since their URLs never change", async ({ page }) => {
  const responses = recordResponses(page);
  const word = await createCardWithImage(page, makePng(BIG.width, BIG.height), "thumb-cache");
  const gridImage = page.locator("li", { hasText: word }).locator("img");
  const src = await gridImage.getAttribute("src");
  await naturalWidth(gridImage); // wait for the browser to have loaded it
  const cacheControl = responses.get(src!)!.headers()["cache-control"] ?? "";
  expect(cacheControl).toMatch(/max-age=31536000/);
  expect(cacheControl).toContain("immutable");
});

test("grid images are lazy-loaded", async ({ page }) => {
  const word = await createCardWithImage(page, makePng(BIG.width, BIG.height), "thumb-lazy");
  await expect(page.locator("li", { hasText: word }).locator("img")).toHaveAttribute(
    "loading",
    "lazy",
  );
});

test("review preloads the next card's image while the current one is answered", async ({
  page,
}) => {
  // The deck holds an imageless card (made by the helper, so it's shown
  // first — oldest first), then two cards with images. While the first is
  // on screen, the next card's preview is already queued (hidden) in the
  // page; grading moves on to a card showing that same, already-fetched URL.
  const { deckName, word: plainWord } = await createCardInOwnDeck(page, "thumb-preload");
  for (const prefix of ["thumb-preload-a", "thumb-preload-b"]) {
    const word = await createCardWithImage(page, makePng(BIG.width, BIG.height), prefix);
    await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
    await withServerResponse(page, "add_card_to_deck", () => page.getByLabel(deckName).check());
  }

  await page.goto("/review");
  await page.getByLabel("Deck").selectOption(deckName);
  await expect(page.getByText("Reveal")).toBeVisible();
  await expect(page.locator(".review-card img")).toHaveCount(0);
  const preloaded = await page.locator("img[hidden]").first().getAttribute("src");
  expect(preloaded).toBeTruthy();

  await page.getByText("Reveal").click();
  await expect(page.getByText(plainWord, { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Good" }).click();

  const shown = page.locator(".review-card img");
  await expect(shown).toBeVisible();
  expect(await shown.getAttribute("src")).toBe(preloaded);
});

test("the backfill makes previews for images stored before previews existed", async ({ page }) => {
  const word = await createCardWithImage(page, makePng(BIG.width, BIG.height), "thumb-backfill");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  const fullSrc = (await page.locator("img").first().getAttribute("src"))!;

  // Put the image back in its "uploaded before previews existed" state.
  await forgetThumbnail(fullSrc);
  expect(await recordedThumbnail(fullSrc)).toBeNull();

  // Batches until nothing is left (other specs' images may be queued too).
  let remaining = 1;
  for (let i = 0; i < 20 && remaining > 0; i++) {
    const res = await page.request.post("/api/admin/backfill-thumbnails?limit=50");
    expect(res.status()).toBe(200);
    remaining = (await res.json()).remaining;
  }
  const thumb = await recordedThumbnail(fullSrc);
  expect(thumb, "the backfill recorded a preview").not.toBeNull();
  expect(thumb).not.toBe(fullSrc);
});

test("the backfill needs a signed-in session", async ({ browser, baseURL }) => {
  const anon = await browser.newContext({ storageState: { cookies: [], origins: [] }, baseURL });
  try {
    const res = await anon.request.post("/api/admin/backfill-thumbnails");
    expect(res.status()).toBe(401);
  } finally {
    await anon.close();
  }
});
