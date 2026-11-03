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

// Links `word` to `nextWord` as a back-chain step (word -> nextWord), from
// word's own edit page — same UI flow card-links.spec.ts already exercises
// for "back-chain step" links. The direction radio is left on its default
// ("this card comes before the one I pick"), so word is stored as the
// earlier step and nextWord as the later one.
async function linkAsBackChainStep(
  page: import("@playwright/test").Page,
  word: string,
  nextWord: string,
): Promise<void> {
  await openEditPage(page, word);
  await page.getByLabel("Link type").selectOption("back_chain_step");
  await page.getByPlaceholder("Search cards to link…").fill(nextWord);
  await withServerResponse(page, "add_card_link", () =>
    page.getByRole("button", { name: nextWord }).click(),
  );
}

test("walking a back-chain from its starting card reveals each step in order, then ends", async ({
  page,
}) => {
  const card1 = await createCard(page, "back-chain-spec-1");
  const card2 = await createCard(page, "back-chain-spec-2");
  const card3 = await createCard(page, "back-chain-spec-3");

  await linkAsBackChainStep(page, card1, card2);
  await linkAsBackChainStep(page, card2, card3);

  await page.goto("/train/back-chain");
  await page.getByRole("button", { name: card1 }).click();
  await expect(page.getByText(card1, { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Next step" }).click();
  await expect(page.getByText(card2, { exact: true })).toBeVisible();
  // The first step is still shown too — the chain accumulates rather than
  // replacing each step.
  await expect(page.getByText(card1, { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Next step" }).click();
  await expect(page.getByText(card3, { exact: true })).toBeVisible();

  await page.getByRole("button", { name: "Next step" }).click();
  await expect(page.getByText("End of chain.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Next step" })).toHaveCount(0);
});
