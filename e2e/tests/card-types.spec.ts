import { Page } from "@playwright/test";
import { test, expect } from "./fixtures";
import { uniqueWord } from "./helpers";

// Which media fields each card type offers. Video and YouTube belong to
// "video_to_written" only; an audio card offers audio and nothing else.
const OFFERED: Record<string, { files: string[]; youtube: boolean }> = {
  image_to_target: { files: ["image"], youtube: false },
  sound_to_written: { files: ["audio"], youtube: false },
  image_to_audio: { files: ["audio", "image"], youtube: false },
  video_to_written: { files: ["video"], youtube: true },
  sentence_translation: { files: [], youtube: false },
};

async function offered(page: Page): Promise<{ files: string[]; youtube: boolean }> {
  const files = await page
    .locator('form input[type="file"]')
    .evaluateAll((els) => els.map((el) => (el as HTMLInputElement).name).sort());
  const youtube = (await page.locator('input[name="youtube_url"]').count()) > 0;
  return { files, youtube };
}

// Creates a bare card of `type` (no attachments) and opens its edit page.
async function createAndOpen(page: Page, type: string, word: string): Promise<string> {
  await page.goto("/new");
  await page.getByLabel("Card type").selectOption(type);
  if (type === "sentence_translation") {
    await page.getByLabel("Known-language sentence").fill("a known sentence");
  }
  await page.getByLabel(/^Target-language/).fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  await expect(page).toHaveURL(/\/cards\/[0-9a-f-]+$/);
  return new URL(page.url()).pathname;
}

for (const [type, expected] of Object.entries(OFFERED)) {
  test(`the new-card form for "${type}" offers only its own media fields`, async ({ page }) => {
    await page.goto("/new");
    await page.getByLabel("Card type").selectOption(type);
    await expect.poll(() => offered(page)).toEqual(expected);
  });

  test(`the edit page of a "${type}" card shows that type and only its media fields`, async ({
    page,
  }) => {
    await createAndOpen(page, type, uniqueWord(`types-${type}`));
    // Regression: the dropdown used to show the first option
    // ("Image → written") for any other type, so what the form displayed
    // contradicted which fields it offered — and saving would have
    // silently re-typed the card.
    await expect(page.getByLabel("Card type")).toHaveValue(type);
    await expect.poll(() => offered(page)).toEqual(expected);
  });
}

test("an audio card's server-rendered form already selects its own type", async ({ page }) => {
  // The page's own JS can paper over a wrong server-rendered <select>, so
  // read the raw HTML the server sent instead of the hydrated DOM — that is
  // what an unhydrated page (or one whose hydration crashed) shows.
  const path = await createAndOpen(page, "sound_to_written", uniqueWord("types-ssr"));
  const html = await (await page.request.get(path)).text();
  const option = (value: string) =>
    new RegExp(`<option(?=[^>]*value="${value}")[^>]*>`).exec(html)?.[0] ?? "";
  expect(option("sound_to_written")).toMatch(/\bselected\b/);
  expect(option("image_to_target")).not.toMatch(/\bselected\b/);
});

test("changing the card type on the form swaps which media fields are offered", async ({
  page,
}) => {
  await page.goto("/new");
  await page.getByLabel("Card type").selectOption("sound_to_written");
  await expect.poll(() => offered(page)).toEqual(OFFERED.sound_to_written);
  await page.getByLabel("Card type").selectOption("video_to_written");
  await expect.poll(() => offered(page)).toEqual(OFFERED.video_to_written);
  await page.getByLabel("Card type").selectOption("sentence_translation");
  await expect.poll(() => offered(page)).toEqual(OFFERED.sentence_translation);
});

test("an attachment the card's type wouldn't offer stays visible so it can be removed", async ({
  page,
}) => {
  const word = uniqueWord("types-existing");
  await page.goto("/new");
  await page.getByLabel("Card type").selectOption("video_to_written");
  await page.getByLabel("Or link a YouTube video").fill("https://youtu.be/dQw4w9WgXcQ");
  await page.getByLabel(/^Target-language/).fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();

  // Re-type the card to one that doesn't offer video/YouTube: the link it
  // already carries must not vanish from the form.
  await page.getByLabel("Card type").selectOption("image_to_target");
  await expect(page.getByLabel("Remove YouTube link")).toBeVisible();
  await expect(page.locator('input[name="image"]')).toBeVisible();
});
