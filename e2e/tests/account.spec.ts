import { test, expect } from "./fixtures";
import { Client } from "pg";
import { uniqueWord, withServerResponse, createInviteToken, RESEND_ALLOWED_EMAIL } from "./helpers";

// Every test here signs up its own throwaway account rather than using
// auth.setup.ts's shared storageState — changing the shared account's
// email would break every other spec file's login, and changing its
// password revokes every session for that user (see account::
// change_password), which would log the shared storageState out from
// under whichever other spec happens to run concurrently. Same reasoning
// auth.spec.ts's "full password reset" test already documents for the
// same hazard.
test.use({ storageState: { cookies: [], origins: [] } });

// sign_up enforces a real, deliberate MAX_USERS = 4 cap (see auth.rs) —
// production safety, not a test-infra accident. auth.setup.ts's shared
// account plus auth.spec.ts's own throwaway reset-flow account already use
// up some of those 4 concurrently; running this file's tests (each of
// which signs up at least one more account) in parallel with each other
// would blow straight through what's left. Serial keeps this file's own
// usage to one account at a time; the try/finally cleanup in every test
// below returns it immediately afterward — including on a failed
// assertion or a Playwright-driven retry, not just the happy path — so
// this file never leaves an account behind to slowly starve the cap on
// whatever database it's run against, including a long-lived preview
// branch that's reused across many pushes rather than reset each time.
test.describe.configure({ mode: "serial" });

async function signUpFreshAccount(
  page: import("@playwright/test").Page,
  prefix: string,
  password = "correct-horse-battery",
): Promise<{ email: string }> {
  const email = `${uniqueWord(prefix)}@example.com`;
  const token = await createInviteToken(email);
  await page.goto(`/signup?token=${token}`);
  await page.getByLabel("Name").fill("Account Spec");
  await page.getByLabel("Password", { exact: true }).fill(password);
  await page.getByLabel("Confirm password").fill(password);
  await page.getByRole("button", { name: "Sign up" }).click();
  await expect(page).toHaveURL("/");
  return { email };
}

// Deletes a test-created user and everything that references it (a fresh
// sign-up always has at least one session row; nothing here has FK
// cascade, so children go first). `email` is matched case-insensitively
// since sign_up/update_email both lowercase on the way in. Safe to call
// with an email that was never actually created (e.g. signup itself
// failed before a test got this far) — the DELETEs just affect zero rows.
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

test("changing your name updates what the form shows after reload", async ({ page }) => {
  const { email } = await signUpFreshAccount(page, "account-name");
  try {
    await page.goto("/account");

    await page.getByLabel("Name").fill("Renamed Person");
    await withServerResponse(page, "update_name", () =>
      page.getByRole("button", { name: "Save" }).first().click(),
    );
    await expect(page.getByText("Error:")).not.toBeVisible();

    await page.goto("/account");
    await expect(page.getByLabel("Name")).toHaveValue("Renamed Person");
  } finally {
    await deleteTestUser(email);
  }
});

test("changing your email requires the correct current password", async ({ page }) => {
  const { email } = await signUpFreshAccount(page, "account-email-wrongpw");
  try {
    await page.goto("/account");

    await page.getByLabel("New email").fill(`${uniqueWord("new")}@example.com`);
    await page.getByLabel("Current password").first().fill("not-the-right-password");
    // No withServerResponse here: the action returns an error (non-ok)
    // response, which that helper's res.ok() check would wait on forever.
    // The assertion below already polls until the error actually renders,
    // which is enough to know the round trip finished before moving on.
    await page.getByRole("button", { name: "Save" }).nth(1).click();
    await expect(page.getByText("incorrect password")).toBeVisible();

    // Unchanged — still reachable at the original email.
    await page.goto("/account");
    await expect(page.getByLabel("New email")).toHaveValue(email);
  } finally {
    await deleteTestUser(email);
  }
});

test("changing your email with the right password updates it and future logins", async ({
  page,
}) => {
  const { email } = await signUpFreshAccount(page, "account-email-ok");
  const newEmail = `${uniqueWord("account-email-ok-new")}@example.com`;
  try {
    await page.goto("/account");

    await page.getByLabel("New email").fill(newEmail);
    await page.getByLabel("Current password").first().fill("correct-horse-battery");
    await withServerResponse(page, "update_email", () =>
      page.getByRole("button", { name: "Save" }).nth(1).click(),
    );
    await expect(page.getByText("Error:")).not.toBeVisible();

    await page.goto("/account");
    await expect(page.getByLabel("New email")).toHaveValue(newEmail);

    // /auth/logout redirects straight to /login.
    await page.goto("/auth/logout");
    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill("correct-horse-battery");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByText("invalid email or password")).toBeVisible();

    await page.getByLabel("Email").fill(newEmail);
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page).toHaveURL("/");
  } finally {
    // The account now lives under newEmail, not the original — delete
    // whichever one it actually ended up at.
    await deleteTestUser(newEmail);
  }
});

test("changing your email to one already in use is rejected", async ({ page }) => {
  // RESEND_ALLOWED_EMAIL (auth.setup.ts's shared account) is already
  // guaranteed to exist for the whole suite's run, so reusing it as "an
  // email already in use" needs only this one throwaway account instead
  // of two live at once — keeping this test's own peak usage at the same
  // one-temp-account budget every other test here holds to, which is what
  // keeps the whole suite under MAX_USERS = 4 when files run concurrently
  // (see the file-level comment above).
  const { email: dupEmail } = await signUpFreshAccount(page, "account-email-dup");
  try {
    await page.goto("/account");

    await page.getByLabel("New email").fill(RESEND_ALLOWED_EMAIL);
    await page.getByLabel("Current password").first().fill("correct-horse-battery");
    // Error response again — see the note in the wrong-password test above.
    await page.getByRole("button", { name: "Save" }).nth(1).click();
    await expect(page.getByText(/already exists/i)).toBeVisible();
  } finally {
    await deleteTestUser(dupEmail);
  }
});

test("changing your password requires the correct current password", async ({ page }) => {
  const { email } = await signUpFreshAccount(page, "account-pw-wrongcurrent");
  try {
    await page.goto("/account");

    await page.getByLabel("Current password").nth(1).fill("not-the-right-password");
    await page.getByLabel("New password", { exact: true }).fill("a-totally-different-password");
    await page.getByLabel("Confirm new password").fill("a-totally-different-password");
    // Error response again — see the note in the email section's
    // wrong-password test above.
    await page.getByRole("button", { name: "Change password" }).click();
    await expect(page.getByText("incorrect password")).toBeVisible();
    // Still on /account, not bounced to /login — the attempt was rejected
    // before any session was touched.
    await expect(page).toHaveURL("/account");
  } finally {
    await deleteTestUser(email);
  }
});

test("changing your password logs you out and lets you sign in with the new one", async ({
  page,
}) => {
  const { email } = await signUpFreshAccount(page, "account-pw-ok");
  try {
    await page.goto("/account");

    await page.getByLabel("Current password").nth(1).fill("correct-horse-battery");
    await page.getByLabel("New password", { exact: true }).fill("a-brand-new-password-99");
    await page.getByLabel("Confirm new password").fill("a-brand-new-password-99");
    // No withServerResponse/goto race here: the redirect to /login is the
    // app's own effect, which only fires once the action's value is
    // already Some(Ok(())) — i.e. strictly after the round trip completes,
    // not a navigation this test script is racing against.
    await page.getByRole("button", { name: "Change password" }).click();

    await expect(page).toHaveURL("/login?reset=success");
    await expect(page.getByText("Password updated")).toBeVisible();

    await page.getByLabel("Email").fill(email);
    await page.getByLabel("Password").fill("correct-horse-battery");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByText("invalid email or password")).toBeVisible();

    await page.getByLabel("Password").fill("a-brand-new-password-99");
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page).toHaveURL("/");
  } finally {
    await deleteTestUser(email);
  }
});
