-- BKR-41/BKR-134: private progress anchored to an immutable ChapterVersion.
-- This is STORAGE ONLY. Inserting a row never grants paid-content access.
-- Read and save operations must check the current purchase/library entitlement,
-- purchase revocation and admin hold in the SAME database statement.
-- Existing free-reader table reader_book_positions remains authoritative for
-- public/free continuation and is deliberately not rewritten here.

CREATE TABLE reader_version_positions (
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    chapter_id BIGINT NOT NULL,
    version_id BIGINT NOT NULL,
    block_index INTEGER NOT NULL CHECK (block_index BETWEEN 1 AND 200),
    char_offset INTEGER NOT NULL CHECK (char_offset BETWEEN 0 AND 100000),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, chapter_id, version_id),
    CONSTRAINT reader_version_positions_chapter_version_fk
        FOREIGN KEY (chapter_id, version_id)
        REFERENCES chapter_versions (chapter_id, id) ON DELETE RESTRICT
);

-- One user's recently read purchased versions can be listed in a bounded query.
CREATE INDEX reader_version_positions_recent_idx
    ON reader_version_positions (user_id, updated_at DESC, chapter_id DESC);

-- Saving a place is monotonic within this exact version: use an authorized
-- INSERT .. SELECT from library_purchased_chapters + purchase facts + revocations
-- and ON CONFLICT (user_id, chapter_id, version_id) DO UPDATE ... WHERE
-- (saved.block_index, saved.char_offset) < (EXCLUDED.block_index, EXCLUDED.char_offset).
-- The application implementing BKR-134 is responsible for this access-gated
-- statement, not this DDL-only migration.
