-- Every `list_cards`/`list_decks` query orders by `cards.created_at` with
-- no supporting index, forcing a full sequential scan + sort over the
-- whole table on every card-list page load. Harmless while the table held
-- a handful of hand-curated cards; measurably slow once the dictionary
-- importer started bulk-adding real vocabulary (confirmed directly via
-- slow-query logs during a Playwright run: this exact query taking
-- several seconds under concurrent load) — not a Neon compute-sizing
-- problem, a missing index on a table that grew.
CREATE INDEX cards_created_at_idx ON cards (created_at DESC);
