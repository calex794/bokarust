-- BKR-41: make it impossible to store a continuation point for a chapter
-- that belongs to a different book. All previous migrations remain unchanged.
-- Existing mismatched rows, if any, must be investigated rather than silently deleted.

-- A primary key on chapters(id) alone cannot be targeted by a composite FK.
ALTER TABLE chapters
    ADD CONSTRAINT chapters_book_id_id_unique UNIQUE (book_id, id);

-- Validate against existing rows. A failed validation aborts this migration,
-- preserving existing data rather than silently changing its meaning.
ALTER TABLE reader_book_positions
    ADD CONSTRAINT reader_book_positions_chapter_book_fk
    FOREIGN KEY (book_id, chapter_id)
    REFERENCES chapters (book_id, id)
    ON DELETE CASCADE
    NOT VALID;

ALTER TABLE reader_book_positions
    VALIDATE CONSTRAINT reader_book_positions_chapter_book_fk;
