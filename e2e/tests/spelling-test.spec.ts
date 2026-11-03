import { test, expect } from "./fixtures";
import { uniqueWord } from "./helpers";

// Round selection in spelling_test_round() is `ORDER BY random() LIMIT 1`
// over every card in the shared pool that has at least one media field —
// not scoped to a deck the way createCardInOwnDeck's review tests are, so
// a spec here can never assume the round it's handed back is the card it
// just created. Each test below only relies on *some* qualifying card
// existing (guaranteed by creating one with a YouTube link — no storage
// backend needed, unlike an uploaded image — right before visiting
// /train/spelling) and otherwise treats the round's actual content as
// unknown.
test("typing the wrong spelling shows the correct answer, and Next loads a fresh round", async ({
  page,
}) => {
  const word = uniqueWord("spelling-test");

  await page.goto("/new");
  await page.getByLabel("Target-language word/phrase").fill(word);
  await page
    .getByLabel("Or link a YouTube video")
    .fill("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
  await page.getByRole("button", { name: "Add" }).click();
  await expect(page).toHaveURL("/");

  await page.goto("/train/spelling");
  await expect(page.locator(".review-card")).toBeVisible();

  await page.getByLabel("Spelling").fill("definitely-not-the-right-spelling");
  await page.getByRole("button", { name: "Submit" }).click();

  await expect(page.getByText("Not quite — the correct spelling is")).toBeVisible();

  await page.getByRole("button", { name: "Next" }).click();

  // A fresh round rendered — the input is cleared and the previous
  // feedback is gone — not necessarily the same card, since selection is
  // random.
  await expect(page.getByLabel("Spelling")).toHaveValue("");
  await expect(page.getByText("Not quite — the correct spelling is")).not.toBeVisible();
  await expect(page.getByText("Correct!")).not.toBeVisible();
  await expect(page.locator(".review-card")).toBeVisible();
});

test("an anonymous visitor is redirected to log in", async ({ browser }) => {
  const anonContext = await browser.newContext({ storageState: { cookies: [], origins: [] } });
  const anonPage = await anonContext.newPage();
  try {
    await anonPage.goto("/train/spelling");
    await anonPage.locator('body[data-hydrated="true"]').waitFor();
    await expect(anonPage).toHaveURL("/login");
  } finally {
    await anonContext.close();
  }
});
