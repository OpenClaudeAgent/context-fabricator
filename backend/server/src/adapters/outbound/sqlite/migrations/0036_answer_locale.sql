ALTER TABLE settings ADD COLUMN answer_locale TEXT
    CHECK (answer_locale IN ('auto', 'en', 'fr', 'de', 'es', 'it', 'pt', 'ja', 'zh'));
