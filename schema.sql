CREATE TABLE IF NOT EXISTS reading_list
(   id              INTEGER PRIMARY KEY NOT NULL
,   url             TEXT    UNIQUE      NOT NULL
    -- Journal date the key was pulled from; YYYY-MM-DD
,   source_date     TEXT                NOT NULL    DEFAULT (date('now'))
    -- Original line in journal, without sanitization
,   original_text   TEXT
    -- Text of the body, without tags etc.
,   body_text       TEXT
    -- NULL if unknown, 0 for "unread" and 1 for "read"
,   read            INTEGER     -- Boolean
);

CREATE TABLE IF NOT EXISTS roundup_contents
(   date    TEXT    NOT NULL    DEFAULT (date('now'))
,   entry   INTEGER
,   FOREIGN KEY (entry) REFERENCES reading_list(id)
,   PRIMARY KEY (date, entry)
);
