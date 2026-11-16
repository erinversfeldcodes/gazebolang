import { test, expect } from "./fixtures";
import { deleteTaggedCards, seedTaggedCards, uniqueWord } from "./helpers";

// 13 cards sharing one tag: 2 pages at 12 per page, 1 at 25.
const COUNT = 13;

test.describe("card list paging", () => {
  let tag: string;

  test.beforeEach(async () => {
    tag = uniqueWord("page-tag");
    await seedTaggedCards(tag, COUNT);
  });

  test.afterEach(async () => {
    await deleteTaggedCards(tag);
  });

  test("shows the current page, the page count and the item range, and lets you page", async ({
    page,
  }) => {
    await page.goto("/");
    await page.getByLabel("Filter by tag").selectOption(tag);
    await page.getByLabel("Cards per page").selectOption("12");

    const pager = page.locator(".card-pager").first();
    await expect(pager.getByText(`Showing 1–12 of ${COUNT}`)).toBeVisible();
    await expect(pager.getByLabel("Page")).toHaveValue("1");
    await expect(pager).toContainText("of 2");
    await expect(pager.getByRole("button", { name: "First" })).toBeDisabled();
    await expect(pager.getByRole("button", { name: "Previous" })).toBeDisabled();
    await expect(page.locator(".card-grid > li")).toHaveCount(12);

    await pager.getByRole("button", { name: "Next" }).click();
    await expect(pager.getByText(`Showing 13–13 of ${COUNT}`)).toBeVisible();
    await expect(page.locator(".card-grid > li")).toHaveCount(1);
    await expect(pager.getByRole("button", { name: "Next" })).toBeDisabled();
    await expect(pager.getByRole("button", { name: "Last" })).toBeDisabled();

    await pager.getByRole("button", { name: "First" }).click();
    await expect(pager.getByText(`Showing 1–12 of ${COUNT}`)).toBeVisible();
    await pager.getByRole("button", { name: "Last" }).click();
    await expect(pager.getByText(`Showing 13–13 of ${COUNT}`)).toBeVisible();
  });

  test("the page box jumps straight to a page and clamps out-of-range numbers", async ({
    page,
  }) => {
    await page.goto("/");
    await page.getByLabel("Filter by tag").selectOption(tag);
    await page.getByLabel("Cards per page").selectOption("12");

    const pager = page.locator(".card-pager").first();
    // Wait for the page-size change to finish re-rendering the pager before
    // typing into it: on a slow backend (the preview stack) the refetch
    // lands after `fill`, replacing the input with a fresh one showing "1"
    // and silently discarding what was typed.
    await expect(pager.getByText(`Showing 1–12 of ${COUNT}`)).toBeVisible();
    await pager.getByLabel("Page").fill("2");
    await pager.getByLabel("Page").press("Enter");
    await expect(pager.getByText(`Showing 13–13 of ${COUNT}`)).toBeVisible();

    await pager.getByLabel("Page").fill("99");
    await pager.getByLabel("Page").press("Enter");
    await expect(pager.getByText(`Showing 13–13 of ${COUNT}`)).toBeVisible();
  });

  test("changing the page size keeps your place and drops the extra pages", async ({ page }) => {
    await page.goto("/");
    await page.getByLabel("Filter by tag").selectOption(tag);
    await page.getByLabel("Cards per page").selectOption("12");
    await page.locator(".card-pager").first().getByRole("button", { name: "Next" }).click();
    await expect(page.getByText(`Showing 13–13 of ${COUNT}`).first()).toBeVisible();

    await page.getByLabel("Cards per page").selectOption("25");
    const pager = page.locator(".card-pager").first();
    await expect(pager.getByText(`Showing 1–${COUNT} of ${COUNT}`)).toBeVisible();
    await expect(pager).toContainText("of 1");
    await expect(page.locator(".card-grid > li")).toHaveCount(COUNT);
    await expect(pager.getByRole("button", { name: "Next" })).toBeDisabled();
  });

  test("the pager is also shown below the cards", async ({ page }) => {
    await page.goto("/");
    await page.getByLabel("Filter by tag").selectOption(tag);
    await expect(page.locator(".card-pager")).toHaveCount(2);
  });
});
