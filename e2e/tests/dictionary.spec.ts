import { Page } from "@playwright/test";
import { test, expect } from "./fixtures";
import { createCardInOwnDeck, uniqueWord, withServerResponse } from "./helpers";

// Saves dictionary details for the word a card was just created with, from
// that card's own edit page. Assumes `page` is already signed in and the
// card exists (e.g. via createCardInOwnDeck).
async function saveDictionaryDetails(
  page: Page,
  word: string,
  details: { partOfSpeech?: string; notes?: string; linkedWords?: string; english?: string },
): Promise<void> {
  await page.goto("/");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  // Dictionary details is an always-visible section now, not a disclosure
  // to expand first.
  if (details.english) {
    await page.getByLabel("English translation").fill(details.english);
  }
  if (details.partOfSpeech) {
    await page.getByLabel("Part of speech").fill(details.partOfSpeech);
  }
  if (details.notes) {
    await page.getByLabel("Notes").fill(details.notes);
  }
  if (details.linkedWords) {
    await page.getByLabel("Related words (comma separated)").fill(details.linkedWords);
  }
  await withServerResponse(page, "save_word_definition", () =>
    page.getByRole("button", { name: "Save dictionary details" }).click(),
  );
  await expect(page.getByText("Saved.")).toBeVisible();
}

// Reveals the one card in `deckName`'s review queue and clicks its word to
// open the dictionary popup — stops short of grading, since the tests here
// care about what the popup shows, not spaced-repetition scheduling.
async function revealAndOpenPopup(
  page: Page,
  deckName: string,
  word: string,
): Promise<void> {
  await page.goto("/review");
  await page.getByLabel("Deck").selectOption(deckName);
  await expect(page.getByText("Reveal")).toBeVisible();
  await page.getByText("Reveal").click();
  await page.locator(".clickable-word", { hasText: word }).click();
}

test("saved dictionary details appear in the popup when reviewing the card", async ({ page }) => {
  const { word, deckName } = await createCardInOwnDeck(page, "dict-spec-basic");
  await saveDictionaryDetails(page, word, { partOfSpeech: "noun", notes: "a test word" });

  await revealAndOpenPopup(page, deckName, word);
  const dialog = page.locator("dialog.word-dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("noun")).toBeVisible();
  await expect(dialog.getByText("a test word")).toBeVisible();
});

test("a word with no saved dictionary entry shows a graceful empty state", async ({ page }) => {
  const { word, deckName } = await createCardInOwnDeck(page, "dict-spec-empty");

  await revealAndOpenPopup(page, deckName, word);
  await expect(page.locator("dialog.word-dialog").getByText("No dictionary entry yet")).toBeVisible();
});

test("clicking a related word swaps the open dialog's content instead of opening a second one", async ({
  page,
}) => {
  const { word: wordA, deckName } = await createCardInOwnDeck(page, "dict-spec-a");
  const { word: wordB } = await createCardInOwnDeck(page, "dict-spec-b");

  await saveDictionaryDetails(page, wordB, { partOfSpeech: "a-distinguishing-part-of-speech" });
  await saveDictionaryDetails(page, wordA, { partOfSpeech: "noun", linkedWords: wordB });

  await revealAndOpenPopup(page, deckName, wordA);
  const dialog = page.locator("dialog.word-dialog");
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText(wordB, { exact: true })).toBeVisible();

  await dialog.locator(".clickable-word", { hasText: wordB }).click();

  // Same dialog, new content — not a second one.
  await expect(page.locator("dialog.word-dialog")).toHaveCount(1);
  await expect(dialog.getByText("a-distinguishing-part-of-speech")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Back" })).toBeVisible();
});

test("opening and closing the popup never disturbs the review grade buttons", async ({ page }) => {
  const { word, deckName } = await createCardInOwnDeck(page, "dict-spec-grading");

  await revealAndOpenPopup(page, deckName, word);
  await expect(page.locator("dialog.word-dialog")).toBeVisible();
  await page.getByRole("button", { name: "Close" }).click();
  await expect(page.locator("dialog.word-dialog")).not.toBeVisible();

  await expect(page.getByRole("button", { name: "Good" })).toBeEnabled();
  await page.getByRole("button", { name: "Good" }).click();
  await page.waitForLoadState("networkidle");
});

// --- the dictionary page ---------------------------------------------------

async function searchDictionary(page: Page, text: string): Promise<void> {
  await page.getByLabel("Search").fill(text);
  await page.getByLabel("Search").press("Enter");
}

test("the dictionary lists a word with its cards, in either direction, and links to its entry", async ({
  page,
}) => {
  const { word } = await createCardInOwnDeck(page, "dict-browse");
  const english = uniqueWord("gloss");
  await saveDictionaryDetails(page, word, { english, partOfSpeech: "noun" });
  // A second card for the same word.
  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.goto("/dictionary");
  await searchDictionary(page, english);
  const row = page.locator(".dictionary-list li", { hasText: word });
  await expect(row).toHaveCount(1);
  await expect(row).toContainText("2 cards");

  // English → target: English is the headword.
  await expect(row.locator(".dictionary-headword")).toHaveText(english);
  await expect(row.locator(".dictionary-other")).toHaveText(word);

  // Target → English: the headword flips.
  await page.getByRole("button", { name: "Target → English" }).click();
  await expect(row.locator(".dictionary-headword")).toHaveText(word);
  await expect(row.locator(".dictionary-other")).toHaveText(english);

  await row.getByRole("link").click();
  await expect(page).toHaveURL(/\/dictionary\/[0-9a-f-]+$/);
  await expect(page.getByRole("heading", { name: word, level: 1 })).toBeVisible();
  await expect(page.getByText(english)).toBeVisible();
  await expect(page.getByText("noun")).toBeVisible();

  // Links to every card for the word.
  const cardLinks = page.locator(".dictionary-cards a");
  await expect(cardLinks).toHaveCount(2);
  await cardLinks.first().click();
  await expect(page).toHaveURL(/\/cards\/[0-9a-f-]+$/);
  await expect(page.getByLabel("Target-language word/phrase")).toHaveValue(word);
});

test("a word with no cards is not in the dictionary list, but is reachable as a related word", async ({
  page,
}) => {
  const { word } = await createCardInOwnDeck(page, "dict-related-main");
  const stub = uniqueWord("dict-stub");
  // Saving a related word nobody has a card for creates a bare entry.
  await saveDictionaryDetails(page, word, { linkedWords: stub, english: uniqueWord("gloss") });

  await page.goto("/dictionary");
  await searchDictionary(page, stub);
  await expect(page.getByText("No words match.")).toBeVisible();

  await page.goto("/dictionary");
  await searchDictionary(page, word);
  await page.locator(".dictionary-list li", { hasText: word }).getByRole("link").click();
  await page.locator(".dictionary-related").getByRole("link", { name: stub }).click();
  await expect(page.getByRole("heading", { name: stub, level: 1 })).toBeVisible();
  await expect(page.getByText("No cards for this word yet.")).toBeVisible();
});

test("searching treats % and _ literally rather than as wildcards", async ({ page }) => {
  await createCardInOwnDeck(page, "dict-wild");
  await page.goto("/dictionary");
  await searchDictionary(page, "%");
  await expect(page.getByText("No words match.")).toBeVisible();
});

test("an unknown dictionary entry says so", async ({ page }) => {
  await page.goto("/dictionary/00000000-0000-0000-0000-000000000000");
  await expect(page.getByText("Word not found.")).toBeVisible();
});
