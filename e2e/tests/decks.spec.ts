import { test, expect } from "./fixtures";
import { uniqueWord, withServerResponse } from "./helpers";

test("creating a deck and assigning a card to it scopes the card list filter", async ({ page }) => {
  const deckName = uniqueWord("decks-spec");
  const word = uniqueWord("decks-spec-card");

  await page.goto("/decks");
  await page.getByLabel("New deck name").fill(deckName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("link", { name: deckName })).toBeVisible();

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  await withServerResponse(page, "add_card_to_deck", () => page.getByLabel(deckName).check());
  await expect(page.getByLabel(deckName)).toBeChecked();

  await page.goto("/");
  await page.getByLabel("Filter by deck").selectOption(deckName);
  // The word is behind each card's "Reveal" control now (see app.rs's
  // CardListPage) — hasText checks textContent, not visibility.
  await expect(page.locator("li", { hasText: word })).toHaveCount(1);
});

test("removing a card from a deck excludes it from that deck's filter again", async ({ page }) => {
  const deckName = uniqueWord("decks-spec-remove");
  const word = uniqueWord("decks-spec-remove-card");

  await page.goto("/decks");
  await page.getByLabel("New deck name").fill(deckName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("link", { name: deckName })).toBeVisible();

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.locator("li", { hasText: word }).getByRole("link", { name: "Edit" }).click();
  await withServerResponse(page, "add_card_to_deck", () => page.getByLabel(deckName).check());
  await expect(page.getByLabel(deckName)).toBeChecked();
  await withServerResponse(page, "remove_card_from_deck", () => page.getByLabel(deckName).uncheck());
  await expect(page.getByLabel(deckName)).not.toBeChecked();

  await page.goto("/");
  await page.getByLabel("Filter by deck").selectOption(deckName);
  await expect(page.locator("li", { hasText: word })).toHaveCount(0);
});

test("checking a deck on the new-card form assigns it without a separate edit step", async ({
  page,
}) => {
  const deckName = uniqueWord("decks-spec-create");
  const word = uniqueWord("decks-spec-create-card");

  await page.goto("/decks");
  await page.getByLabel("New deck name").fill(deckName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("link", { name: deckName })).toBeVisible();

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByLabel(deckName).check();
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.getByLabel("Filter by deck").selectOption(deckName);
  await expect(page.locator("li", { hasText: word })).toHaveCount(1);

  // Additive, not a replacement for the automatic default-deck membership
  // every card still gets regardless of which other decks were checked.
  await page.getByLabel("Filter by deck").selectOption("Default");
  await expect(page.locator("li", { hasText: word })).toHaveCount(1);
});

test("a newly created card automatically lands in the Default deck", async ({ page }) => {
  const word = uniqueWord("decks-spec-default");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.getByLabel("Filter by deck").selectOption("Default");
  await expect(page.locator("li", { hasText: word })).toHaveCount(1);
});

test("renaming a deck updates its name", async ({ page }) => {
  const deckName = uniqueWord("decks-spec-rename");
  const newName = uniqueWord("decks-spec-renamed");

  await page.goto("/decks");
  await page.getByLabel("New deck name").fill(deckName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("link", { name: deckName })).toBeVisible();

  const row = page.locator("li", { hasText: deckName });
  await row.locator('input[name="name"]').fill(newName);
  await row.getByRole("button", { name: "Rename" }).click();

  await expect(page.getByRole("link", { name: newName })).toBeVisible();
  await expect(page.getByRole("link", { name: deckName, exact: true })).not.toBeVisible();
});

test("deleting a deck removes it from the list", async ({ page }) => {
  const deckName = uniqueWord("decks-spec-delete");

  await page.goto("/decks");
  await page.getByLabel("New deck name").fill(deckName);
  await page.getByRole("button", { name: "Create" }).click();
  await expect(page.getByRole("link", { name: deckName })).toBeVisible();

  // Delete requires an arming click before the confirming one — see
  // app.rs's confirm_button.
  const row = page.locator("li", { hasText: deckName });
  await row.getByRole("button", { name: "Delete" }).click();
  await row.getByRole("button", { name: "Confirm delete?" }).click();

  await expect(page.getByRole("link", { name: deckName })).not.toBeVisible();
});
