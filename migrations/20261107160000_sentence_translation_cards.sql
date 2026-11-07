-- sentence_translation cards are plain known-language -> target-language
-- sentence pairs (no media at all). target_text already holds the
-- target-language side for every card type; source_text is the new,
-- type-specific known-language side. Nullable: every other card_type
-- leaves it unset.
ALTER TABLE cards ADD COLUMN source_text TEXT;
