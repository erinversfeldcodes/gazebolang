-- Mirrors minimal_pair_attempts exactly — same graded-discrimination-drill
-- shape, just for the sound_spelling link type instead of minimal_pair.
CREATE TABLE sound_spelling_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id),
    card_a_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    card_b_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    picked_card_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    correct BOOLEAN NOT NULL,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX sound_spelling_attempts_user_id_idx ON sound_spelling_attempts (user_id, attempted_at DESC);
