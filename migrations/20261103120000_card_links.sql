-- Typed card-to-card relationship links (minimal pairs, back-chaining
-- sequences, sound-spelling correspondences) — the card-level analog of
-- word_links. link_type is part of the PK, not just the pair, because the
-- same two cards can legitimately hold more than one relationship type at
-- once (e.g. both a minimal pair and a sound-spelling correspondence).
--
-- Storage order is never canonicalized by sorting the two UUIDs: for the
-- directional type (back_chain_step), card_id is the earlier step and
-- linked_card_id is the later one, and canonicalizing would silently
-- destroy that order. A reverse-order duplicate of the same (pair, type)
-- is instead blocked by the expression unique index below, enforced
-- atomically by Postgres rather than an application-level check-then-insert
-- (which would race under concurrent writes).
--
-- v1 accepted gap, same spirit as this schema's other documented
-- simplifications: nothing here stops a card from having more than one
-- outgoing/incoming back_chain_step edge, i.e. the edges aren't constrained
-- to form a simple chain. A future back-chaining feature walking this
-- table has to assume (or itself enforce) a simple path.
CREATE TABLE card_links (
    card_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    linked_card_id UUID NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    link_type TEXT NOT NULL,
    PRIMARY KEY (card_id, linked_card_id, link_type)
);

CREATE INDEX card_links_linked_card_id_idx ON card_links (linked_card_id);

CREATE UNIQUE INDEX card_links_unordered_pair_idx
    ON card_links (LEAST(card_id, linked_card_id), GREATEST(card_id, linked_card_id), link_type);
