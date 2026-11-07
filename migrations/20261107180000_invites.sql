-- Shaped like password_reset_tokens: a single-use (used_at), time-limited
-- (expires_at) token. email is fixed at invite-creation time — the whole
-- point of an invite is that sign-up binds to this email, never an
-- arbitrary one typed into the signup form.
CREATE TABLE invites (
    token TEXT PRIMARY KEY,
    email TEXT NOT NULL,
    invited_by UUID NOT NULL REFERENCES users(id),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
