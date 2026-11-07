-- card_type has been a flat, unconstrained TEXT column since the original
-- cards migration (see its comment), written as a hardcoded literal and
-- never read back. It's about to become a real, selectable discriminator
-- (Rust-side: a CardType enum mirroring Direction's as_str()/from_db()
-- round-trip) now that a second variant — sound_to_written, an audio-only
-- prompt reviewed the same way an image_to_target card's audio already
-- is — actually exists. The CHECK documents the now-enforced set of
-- values without promoting it to a Postgres enum, keeping the same
-- flat-TEXT convention word_dictionary_entries.part_of_speech already
-- uses for the same reason (cheap to extend later without an ALTER TYPE).
ALTER TABLE cards ADD CONSTRAINT cards_card_type_check
    CHECK (card_type IN ('image_to_target', 'sound_to_written', 'sentence_translation'));
