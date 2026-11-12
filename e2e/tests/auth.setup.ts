import { test as setup, expect } from "./fixtures";
import { Client } from "pg";
import * as fs from "fs";
import { RESEND_ALLOWED_EMAIL, TEST_PASSWORD, createInviteToken } from "./helpers";

const TEST_EMAIL = RESEND_ALLOWED_EMAIL;

// Signs up via an invite (or, if this suite already ran against this
// database, logs in — signup is now the only way to create an account at
// all, and an invite's email is fixed at mint time, so there's no "repeat
// signup" rejection to fall back from here) as the one shared test account
// every other spec file's storageState depends on. Replaces the old
// magic-link version of this setup, which bypassed the slow part (actually
// receiving an email) by reading the login token straight out of Postgres
// instead of out of an inbox; this bypasses the equivalent slow part for
// invites the same way, via createInviteToken.
setup("authenticate", async ({ page }) => {
  // Fresh database (every CI run, since a PR's preview Neon branch is
  // reset from staging): no account exists yet, so mint an invite and
  // sign up through it. A database this suite already ran against
  // locally: the account already exists — log in with the same
  // credentials instead, since redeeming a second invite for the same
  // email isn't a thing (sign_up_via_invite binds to the invite's own
  // email, but nothing stops minting one for an email that already has an
  // account — redemption would just fail with "an account with that email
  // already exists" — so check for the account directly instead of
  // relying on that error path).
  if (await testEmailHasAccount()) {
    await page.goto("/login");
    await page.getByLabel("Email").fill(TEST_EMAIL);
    await page.getByLabel("Password", { exact: true }).fill(TEST_PASSWORD);
    await page.getByRole("button", { name: "Sign in" }).click();
  } else {
    const token = await createInviteToken(TEST_EMAIL);
    await page.goto(`/signup?token=${token}`);
    await page.getByLabel("Name").fill("Test User");
    await page.getByLabel("Password", { exact: true }).fill(TEST_PASSWORD);
    await page.getByLabel("Confirm password").fill(TEST_PASSWORD);
    await page.getByRole("button", { name: "Sign up" }).click();
  }

  // Generous timeout for consistency with every other login/signup round
  // trip in the suite — auth.rs bounds Argon2 work to 2 concurrent
  // operations, so this can occasionally queue behind other specs' setup.
  await expect(page).toHaveURL("/", { timeout: 15_000 });

  // The daily new-card limit is real per-user product behavior (paces how
  // many unseen cards a learner is introduced to), not something the e2e
  // suite is testing — that's covered precisely, in isolation, by
  // crates/web/src/db.rs's #[sqlx::test] suite. Uncapping it here is just
  // a safety margin on top of helpers.ts's deck-scoped review pattern, so
  // no spec can ever be starved of a "new" card by the shared pool's
  // accumulated history.
  await removeDailyNewCardLimit(TEST_EMAIL);

  fs.mkdirSync(".auth", { recursive: true });
  await page.context().storageState({ path: ".auth/user.json" });
});

async function removeDailyNewCardLimit(email: string): Promise<void> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    await client.query("UPDATE users SET daily_new_card_limit = NULL WHERE email = $1", [email]);
  } finally {
    await client.end();
  }
}

async function testEmailHasAccount(): Promise<boolean> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const result = await client.query("SELECT 1 FROM users WHERE email = $1", [
      TEST_EMAIL.toLowerCase(),
    ]);
    return result.rows.length > 0;
  } finally {
    await client.end();
  }
}
