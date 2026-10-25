-- BKR-41: independent author ACL and working-team membership.
-- Membership alone does not grant authority to edit a Book; team readers are
-- governed by a separate, revocable intersection of active membership/book link.

CREATE TABLE book_members (
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK (role IN ('owner','editor','translator','proofreader')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ,
    PRIMARY KEY(book_id, user_id),
    CHECK (ended_at IS NULL OR ended_at >= joined_at)
);
CREATE UNIQUE INDEX book_members_single_owner_idx
    ON book_members(book_id) WHERE role = 'owner' AND ended_at IS NULL;
CREATE INDEX book_members_active_by_user_idx
    ON book_members(user_id, book_id) WHERE ended_at IS NULL;

CREATE TABLE teams (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    owner_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (char_length(btrim(name)) BETWEEN 1 AND 200),
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE team_members (
    team_id BIGINT NOT NULL REFERENCES teams(id) ON DELETE RESTRICT,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK (role IN ('owner','admin','member')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ,
    PRIMARY KEY(team_id, user_id),
    CHECK (ended_at IS NULL OR ended_at >= joined_at)
);
CREATE INDEX team_members_active_by_user_idx
    ON team_members(user_id,team_id) WHERE ended_at IS NULL;

CREATE TABLE team_book_links (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    team_id BIGINT NOT NULL REFERENCES teams(id) ON DELETE RESTRICT,
    book_id BIGINT NOT NULL REFERENCES books(id) ON DELETE RESTRICT,
    linked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ,
    CHECK (ended_at IS NULL OR ended_at >= linked_at)
);
CREATE UNIQUE INDEX team_book_links_one_active_team_per_book_idx
    ON team_book_links(book_id) WHERE ended_at IS NULL;
CREATE INDEX team_book_links_active_by_team_idx
    ON team_book_links(team_id,book_id) WHERE ended_at IS NULL;

-- Existing books.owner_user_id remains the owner authority until a separately
-- tested cutover. Do not backfill book_members with fabricated historical rows.
-- Team-to-Book attach/transfer requires a locked transaction and server-side ACL.
