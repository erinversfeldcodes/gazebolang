import { test, expect } from "./fixtures";
import { Client } from "pg";
import { RESEND_ALLOWED_EMAIL, uniqueWord, createInviteToken } from "./helpers";

// Runs signed out, deliberately ignoring the project's saved auth state.
test.use({ storageState: { cookies: [], origins: [] } });

// Browsing the shared card pool is public; this also doubles as proof the
// page doesn't error out with no session cookie at all.
test("visiting the card list while signed out shows it, not a login redirect", async ({
  page,
}) => {
  await page.goto("/");
  await expect(page).toHaveURL("/");
  await expect(page.getByRole("heading", { name: "Cards" })).toBeVisible();
});

test("visiting decks while signed out shows them, not a login redirect", async ({ page }) => {
  await page.goto("/decks");
  await expect(page).toHaveURL("/decks");
  await expect(page.getByRole("heading", { name: "Decks" })).toBeVisible();
  // Mutating decks is still auth-only — no create form for an anonymous visitor.
  await expect(page.getByLabel("New deck name")).not.toBeVisible();
});

for (const path of ["/new", "/review", "/stats", "/settings"]) {
  test(`visiting ${path} while signed out redirects to /login`, async ({ page }) => {
    await page.goto(path);
    await expect(page).toHaveURL("/login");
  });
}

test("requesting a password reset shows a confirmation, not an error", async ({ page }) => {
  await page.goto("/forgot-password");
  // Not a unique per-run address: this exercises a real send through
  // Resend's sandbox, which only accepts the account owner's own exact
  // address (see helpers.ts). The confirmation message is deliberately
  // generic — it never reveals whether the email actually matched an
  // account — so reusing auth.setup.ts's already-registered address
  // still exercises this test's actual assertion.
  await page.getByLabel("Email").fill(RESEND_ALLOWED_EMAIL);
  await page.getByRole("button", { name: "Send reset link" }).click();
  await expect(
    page.getByText("If an account with that email exists, we've sent a password reset link."),
  ).toBeVisible();
});

test("requesting a password reset for an unknown email shows the same confirmation", async ({
  page,
}) => {
  await page.goto("/forgot-password");
  await page.getByLabel("Email").fill(`nobody-${Date.now()}@example.com`);
  await page.getByRole("button", { name: "Send reset link" }).click();
  await expect(
    page.getByText("If an account with that email exists, we've sent a password reset link."),
  ).toBeVisible();
});

test("signing up with a too-short password shows an error, not a silent login", async ({
  page,
}) => {
  const token = await createInviteToken(`${uniqueWord("shorty")}@example.com`);
  await page.goto(`/signup?token=${token}`);
  await page.getByLabel("Name").fill("Shorty");
  await page.getByLabel("Password", { exact: true }).fill("short1");
  await page.getByLabel("Confirm password").fill("short1");
  await page.getByRole("button", { name: "Sign up" }).click();
  await expect(page.getByText(/at least 8 characters/i)).toBeVisible();
  await expect(page).toHaveURL(/\/signup/);
});

// Full round trip against a fresh account created just for this test —
// deliberately *not* RESEND_ALLOWED_EMAIL/auth.setup.ts's shared user:
// confirming a reset revokes every existing session for that user, which
// would log out every other spec's saved storageState if run against the
// account they all share. Reading the token straight out of Postgres
// instead of out of an inbox follows the same precedent auth.setup.ts
// used to use for magic-link tokens — this app has no test-only route for
// it, and doesn't need one.
//
// The account this test creates is deleted again at the end (try/finally,
// so a failed assertion or Playwright's CI retry still cleans up) — left
// alone, it used to just sit there forever. That mattered more than it
// looked like it would: sign_up enforces a real MAX_USERS = 4 cap, and a
// PR's preview Neon branch is reused across every push rather than reset,
// so a handful of past runs' orphaned accounts here were enough to
// silently eat the entire cap and fail every subsequent signup in CI,
// including unrelated specs.
test("a full password reset lets you log in with the new password", async ({ page }) => {
  const email = `${uniqueWord("reset-flow")}@example.com`;
  const originalPassword = "the-original-password";
  const newPassword = "a-brand-new-password";

  const token = await createInviteToken(email);
  await page.goto(`/signup?token=${token}`);
  await page.getByLabel("Name").fill("Reset Flow");
  await page.getByLabel("Password", { exact: true }).fill(originalPassword);
  await page.getByLabel("Confirm password").fill(originalPassword);
  await page.getByRole("button", { name: "Sign up" }).click();
  // Default 5s is too tight here: auth.rs bounds Argon2 work to 2
  // concurrent operations (to avoid OOMing the small preview app), so a
  // burst of signups/logins across the suite can briefly queue behind
  // each other — not a UI bug, the request is just still in flight. Same
  // reasoning applies to every login/signup/reset round trip below.
  await expect(page).toHaveURL("/", { timeout: 15_000 });

  try {
    await page.goto("/forgot-password");
    await page.getByLabel("Email").fill(email);
    await page.getByRole("button", { name: "Send reset link" }).click();
    await expect(
      page.getByText("If an account with that email exists, we've sent a password reset link."),
    ).toBeVisible();

    const token = await fetchLatestResetToken(email);
    await page.goto(`/reset-password?token=${token}`);
    await page.getByLabel("New password", { exact: true }).fill(newPassword);
    await page.getByLabel("Confirm new password").fill(newPassword);
    await page.getByRole("button", { name: "Reset password" }).click();
    await expect(page).toHaveURL("/login?reset=success", { timeout: 15_000 });

    // The old password no longer works…
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password", { exact: true }).fill(originalPassword);
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByText("invalid email or password")).toBeVisible({ timeout: 15_000 });

    // …but the new one does.
    await page.getByLabel("Password", { exact: true }).fill(newPassword);
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page).toHaveURL("/", { timeout: 15_000 });
  } finally {
    await deleteTestUser(email);
  }
});

async function deleteTestUser(email: string): Promise<void> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const lowered = email.toLowerCase();
    await client.query(
      "DELETE FROM sessions WHERE user_id = (SELECT id FROM users WHERE email = $1)",
      [lowered],
    );
    await client.query(
      "DELETE FROM password_reset_tokens WHERE user_id = (SELECT id FROM users WHERE email = $1)",
      [lowered],
    );
    await client.query("DELETE FROM invites WHERE email = $1", [lowered]);
    await client.query("DELETE FROM users WHERE email = $1", [lowered]);
  } finally {
    await client.end();
  }
}

async function fetchLatestResetToken(email: string): Promise<string> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const result = await client.query(
      "SELECT prt.token FROM password_reset_tokens prt " +
        "JOIN users u ON u.id = prt.user_id " +
        "WHERE u.email = $1 ORDER BY prt.created_at DESC LIMIT 1",
      [email],
    );
    if (result.rows.length === 0) {
      throw new Error(`no password reset token found for ${email}`);
    }
    return result.rows[0].token as string;
  } finally {
    await client.end();
  }
}
