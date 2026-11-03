import { test, expect } from "./fixtures";
import { uniqueWord, withServerResponse } from "./helpers";

async function createCard(page: import("@playwright/test").Page, prefix: string): Promise<string> {
  const word = uniqueWord(prefix);
  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");
  return word;
}

async function openEditPage(page: import("@playwright/test").Page, word: string): Promise<void> {
  await page.goto("/");
  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
}

test("linking two cards as a minimal pair shows the link on both cards, and it can be removed", async ({
  page,
}) => {
  const wordA = await createCard(page, "card-links-spec-a");
  const wordB = await createCard(page, "card-links-spec-b");

  await openEditPage(page, wordA);
  // "Minimal pair" is LINK_TYPES[0] — the <select>'s default, so no need
  // to pick it explicitly.
  await page.getByPlaceholder("Search cards to link…").fill(wordB);
  await withServerResponse(page, "add_card_link", () =>
    page.getByRole("button", { name: wordB }).click(),
  );
  await expect(page.locator("li", { hasText: wordB })).toContainText("Minimal pair");

  // The link is symmetric — visiting the other card shows it too.
  await openEditPage(page, wordB);
  await expect(page.locator("li", { hasText: wordA })).toContainText("Minimal pair");

  await withServerResponse(page, "remove_card_link", () =>
    page.locator("li", { hasText: wordA }).getByRole("button", { name: "Remove" }).click(),
  );
  await expect(page.locator("li", { hasText: wordA })).toHaveCount(0);
  await expect(page.getByText("Not linked to any other card.")).toBeVisible();
});

test("a back-chain step link renders the opposite direction arrow on each side", async ({ page }) => {
  const earlierStep = await createCard(page, "card-links-spec-earlier");
  const laterStep = await createCard(page, "card-links-spec-later");

  // From the earlier step's own edit page, pick "Back-chain step" and
  // leave the direction radio on its default ("this card comes before
  // the one I pick") — i.e. earlierStep -> laterStep.
  await openEditPage(page, earlierStep);
  await page.getByLabel("Link type").selectOption("back_chain_step");
  await page.getByPlaceholder("Search cards to link…").fill(laterStep);
  await withServerResponse(page, "add_card_link", () =>
    page.getByRole("button", { name: laterStep }).click(),
  );
  // earlierStep was the source of the link, so it gets the "→" (next step) arrow.
  await expect(page.locator("li", { hasText: laterStep })).toContainText(`→ ${laterStep}`);

  // laterStep is the target of the link, so from its own edit page it
  // gets the "←" (previous step) arrow instead — this is the regression
  // guard for `is_source` actually surviving the round trip, not just
  // being collapsed away on the read side.
  await openEditPage(page, laterStep);
  await expect(page.locator("li", { hasText: earlierStep })).toContainText(`← ${earlierStep}`);
});
