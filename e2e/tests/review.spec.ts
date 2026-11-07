import { test, expect } from "./fixtures";
import { uniqueWord, createCardInOwnDeck, reviewOwnCard, recordClip, withServerResponse } from "./helpers";

test("grading a due card removes it from today's review queue", async ({ page }) => {
  const { word, deckName } = await createCardInOwnDeck(page, "review-spec");

  await reviewOwnCard(page, deckName, word);
  // reviewOwnCard asserts the card was actually offered and graded; that the
  // deck-scoped queue then shows "Nothing due" (rather than re-offering the
  // same card) is the UI-visible proof grading took effect. The exact
  // scheduling math — that "Good" moves due_at a day out, so this card
  // specifically won't reappear today — is asserted precisely and
  // non-flakily by crates/web/src/db.rs's
  // grading_removes_card_from_due_review_until_its_next_interval test
  // against an isolated database, rather than re-proven here.
  await page.goto("/review");
  await page.getByLabel("Deck").selectOption(deckName);
  await expect(page.getByText("Nothing due. Nice.")).toBeVisible();
});

// The exact ordering math (hardest-first sorts by lapses/difficulty, due-date sorts
// by due_at) is covered by crates/web/src/db.rs's #[sqlx::test] suite against
// isolated databases, where it can be asserted precisely. Here we only need
// to prove the dropdown is wired up: switching it re-queries without erroring
// and still shows a due/new count line, whatever cards are in the shared
// pool from other specs running concurrently.
test("switching review order re-queries the queue without erroring", async ({ page }) => {
  await page.goto("/review");
  for (const option of ["random", "hardest", "due"]) {
    await page.getByLabel("Order").selectOption(option);
    await expect(page.getByText("new available today")).toBeVisible();
  }
});

test("switching the deck filter re-queries the queue without erroring", async ({ page }) => {
  const deckName = uniqueWord("review-spec-deck");

  await page.goto("/decks");
  await page.getByLabel("New deck name").fill(deckName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("link", { name: deckName })).toBeVisible();

  await page.goto("/review");
  await page.getByLabel("Deck").selectOption(deckName);
  await expect(page.getByText("new available today")).toBeVisible();

  await page.getByLabel("Deck").selectOption("All decks");
  await expect(page.getByText("new available today")).toBeVisible();
});

// A unique, never-reused tag guarantees this card is the only thing the
// filter can possibly match — the shared card pool has concurrent specs
// adding cards of their own at the same time, so isolation has to come
// from the tag being unique, the same way createCardInOwnDeck isolates by
// deck instead.
test("filtering the review queue by tag narrows it to the tagged card", async ({ page }) => {
  const word = uniqueWord("review-spec-tag-filter");
  const tag = uniqueWord("review-spec-tag");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByLabel("Tags (comma separated)").fill(tag);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.goto("/review");
  await page.getByLabel("Filter by tag").selectOption(tag);
  await expect(page.getByText("Reveal")).toBeVisible();
  await page.getByText("Reveal").click();
  await expect(page.getByText(word, { exact: true })).toBeVisible();
  // Without this, the immediate page.goto below races the grade_card fetch
  // and can cancel it mid-flight (see withServerResponse's own comment in
  // helpers.ts) — leaving the card still due and this assertion flaky.
  await withServerResponse(page, "grade_card", () =>
    page.getByRole("button", { name: "Good" }).click(),
  );

  await page.goto("/review");
  await page.getByLabel("Filter by tag").selectOption(tag);
  await expect(page.getByText("Nothing due. Nice.")).toBeVisible();
});

// sentence_translation has no media at all — source_text fills the
// prompt slot a media_view would otherwise occupy, shown up front rather
// than behind Reveal, with target_text behind Reveal as the answer (see
// app.rs's ReviewPage media_view branch).
test("a sentence-translation card shows its known-language sentence as the prompt", async ({
  page,
}) => {
  const sourceText = uniqueWord("review-spec-source");
  const targetText = uniqueWord("review-spec-target");
  const tag = uniqueWord("review-spec-sentence-tag");

  await page.goto("/new");
  await page.getByLabel("Card type").selectOption("sentence_translation");
  await page.getByLabel("Target-language sentence").fill(targetText);
  await page.getByLabel("Known-language sentence").fill(sourceText);
  await page.getByLabel("Tags (comma separated)").fill(tag);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.goto("/review");
  await page.getByLabel("Filter by tag").selectOption(tag);
  await expect(page.getByText(sourceText, { exact: true })).toBeVisible();
  await page.getByText("Reveal").click();
  await expect(page.getByText(targetText, { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Good" }).click();
});

// sound_to_written only shows an audio field on /new (MediaAttachmentFields'
// audio_only prop) — this proves card_type actually round-trips through
// create_card/the review query rather than just being accepted and
// forgotten, which is exactly what it did before this type existed as a
// plumbed-through, selectable value (see jolly-bouncing-bachman.md's
// plan, part 2).
test("a sound-to-written card created with only audio reviews correctly", async ({ page }) => {
  const word = uniqueWord("review-spec-sound-written");
  const tag = uniqueWord("review-spec-sound-written-tag");

  await page.goto("/new");
  await page.getByLabel("Card type").selectOption("sound_to_written");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await recordClip(page, "audio");
  await page.getByLabel("Tags (comma separated)").fill(tag);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.goto("/review");
  await page.getByLabel("Filter by tag").selectOption(tag);
  const round = page.locator(".review-card");
  await expect(round.locator("audio")).toBeVisible();
  await round.getByText("Reveal").click();
  await expect(page.getByText(word, { exact: true })).toBeVisible();
  await round.getByRole("button", { name: "Good" }).click();
});
