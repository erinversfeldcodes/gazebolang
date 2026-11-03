-- Logs every spelling-test attempt (one row per submitted guess), so
-- per-user spelling-test stats can be built later without needing
-- anything beyond what's already captured here.
CREATE TABLE spelling_test_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id),
    card_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    correct BOOLEAN NOT NULL,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX spelling_test_attempts_user_id_idx ON spelling_test_attempts (user_id, attempted_at DESC);
