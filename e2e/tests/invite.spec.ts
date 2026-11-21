import { test, expect } from "./fixtures";
import { Client } from "pg";
import { uniqueWord, deleteInvitedTestUser, withServerCall, waitForHydration } from "./helpers";

// Exercises the real create_invite -> send_invite -> sign_up_via_invite
// path end to end, through the actual UI on both ends, using the signed-in
// shared test account (auth.setup.ts's default storageState) as the
// inviter. The email send itself can't be observed as a real delivery:
// every environment that sends mail (preview, prod) uses Resend's sandbox
// sender, which refuses anyone but the account owner's own verified
// address as a recipient (see helpers.ts's RESEND_ALLOWED_EMAIL) — so,
// same precedent auth.spec.ts's password-reset test already set, this
// reads the invite token straight out of Postgres instead of out of an
// inbox. create_invite still has to actually run and write a redeemable
// row for that read to find anything, so the email failure is itself a
// meaningful part of what this proves, not a bypass of it.
test("creating an invite and signing up through it creates a working account", async ({
  page,
  browser,
}) => {
  const email = `${uniqueWord("invite-spec")}@example.com`;
  try {
    await page.goto("/account");
    await page.getByLabel("Their email").fill(email);
    await page.getByRole("button", { name: "Send invite" }).click();
    await expect(
      page.getByText(/Invite sent\.|invite created, but the email failed to send/),
    ).toBeVisible();

    const token = await fetchLatestInviteToken(email);

    const signupContext = await browser.newContext({
      storageState: { cookies: [], origins: [] },
    });
    const signupPage = await signupContext.newPage();
    try {
      await signupPage.goto(`/signup?token=${token}`);
      // A page from browser.newContext() is not the fixture's page, so its
      // goto does not wait for hydration. Clicking "Sign up" before the wasm
      // has taken over the form posts it natively: the server signs the
      // account up, then 302s back to /signup?token=... (run 44866377910's
      // trace), and the test sees a non-2xx answer and no redirect to "/".
      await waitForHydration(signupPage);
      await signupPage.getByLabel("Name").fill("Invited Friend");
      await signupPage.getByLabel("Password", { exact: true }).fill("a-friends-password-99");
      await signupPage.getByLabel("Confirm password").fill("a-friends-password-99");
      // Wait for the server's answer, then assert with the original 15s bound.
      const signup = await withServerCall(signupPage, "sign_up_via_invite", () =>
        signupPage.getByRole("button", { name: "Sign up" }).click(),
      );
      expect(signup.ok(), "sign-up request failed").toBe(true);
      await expect(signupPage).toHaveURL("/", { timeout: 15_000 });

      // Signed in as the brand-new account, not left anonymous — and
      // bound to the invite's own email, never whatever the form itself
      // might have tried to claim (there's no email field on this form
      // at all any more, which is the point).
      await signupPage.goto("/account");
      await expect(signupPage.getByLabel("Name")).toHaveValue("Invited Friend");
      await expect(signupPage.getByLabel("New email")).toHaveValue(email);
    } finally {
      await signupContext.close();
    }

    // The invite is now used — redeeming the same token again is
    // rejected, not silently accepted into a second account.
    const reuseContext = await browser.newContext({
      storageState: { cookies: [], origins: [] },
    });
    const reusePage = await reuseContext.newPage();
    try {
      await reusePage.goto(`/signup?token=${token}`);
      await waitForHydration(reusePage);
      await reusePage.getByLabel("Name").fill("Second Attempt");
      await reusePage.getByLabel("Password", { exact: true }).fill("another-password-99");
      await reusePage.getByLabel("Confirm password").fill("another-password-99");
      await reusePage.getByRole("button", { name: "Sign up" }).click();
      await expect(reusePage.getByText(/already been used/i)).toBeVisible();
    } finally {
      await reuseContext.close();
    }
  } finally {
    await deleteInvitedTestUser(email);
  }
});

async function fetchLatestInviteToken(email: string): Promise<string> {
  const client = new Client({ connectionString: process.env.DATABASE_URL });
  await client.connect();
  try {
    const result = await client.query(
      "SELECT token FROM invites WHERE email = $1 ORDER BY created_at DESC LIMIT 1",
      [email.toLowerCase()],
    );
    if (result.rows.length === 0) {
      throw new Error(`no invite found for ${email}`);
    }
    return result.rows[0].token as string;
  } finally {
    await client.end();
  }
}
