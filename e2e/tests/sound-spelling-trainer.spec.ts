import { test, expect } from "./fixtures";
import { recordClip, uniqueWord, withServerResponse } from "./helpers";

async function createCardWithAudio(page: import("@playwright/test").Page, prefix: string): Promise<string> {
  const word = uniqueWord(prefix);
  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  // image_to_target (the default card type) no longer shows an audio
  // field — see media-cards.spec.ts's "recording audio attaches..." test
  // for why. "Audio → written" is the type that still offers it.
  await page.getByLabel("Card type").selectOption("sound_to_written");
  await recordClip(page, "audio");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");
  return word;
}

async function openEditPage(page: import("@playwright/test").Page, word: string): Promise<void> {
  await page.goto("/");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
}

test("a sound-spelling pair linked from two audio cards is drillable in the trainer", async ({
  page,
}) => {
  const wordA = await createCardWithAudio(page, "sound-spelling-spec-a");
  const wordB = await createCardWithAudio(page, "sound-spelling-spec-b");

  await openEditPage(page, wordA);
  // Unlike minimal_pair (LINK_TYPES[0], the <select>'s default),
  // sound_spelling is LINK_TYPES[2] — has to be picked explicitly.
  await page.getByLabel("Link type").selectOption("sound_spelling");
  await page.getByPlaceholder("Search cards to link…").fill(wordB);
  await withServerResponse(page, "add_card_link", () =>
    page.getByRole("button", { name: wordB }).click(),
  );
  await expect(page.locator("li", { hasText: wordB })).toContainText(
    "Sound-spelling correspondence",
  );

  await page.goto("/train/sound-spelling");

  // At least one sound-spelling pair with audio on both sides now exists
  // (the one just created above), so a round renders rather than the
  // empty state. The round actually served isn't asserted to be *this
  // specific* pair — sound_spelling_round() picks randomly across every
  // qualifying pair in the shared pool, so asserting on exact option text
  // would make this test depend on no other such pair existing anywhere
  // else (same precedent as minimal-pair-trainer.spec.ts).
  const round = page.locator(".review-card");
  await expect(round.locator("audio")).toBeVisible();
  const options = round.getByRole("button");
  await expect(options).toHaveCount(2);

  await withServerResponse(page, "submit_sound_spelling_attempt", () => options.first().click());

  // Both options are disabled immediately after a pick, so a second
  // attempt on the same round isn't possible.
  await expect(options.first()).toBeDisabled();
  await expect(options.nth(1)).toBeDisabled();

  await expect(page.getByText(/^(Correct!|Not quite — it was .+\.)$/)).toBeVisible();

  // "Next" bumps round_refresh and fetches a fresh round.
  await round.getByRole("button", { name: "Next" }).click();
  await expect(round.locator("audio")).toBeVisible();
  await expect(options).toHaveCount(2);
  await expect(options.first()).toBeEnabled();
});
