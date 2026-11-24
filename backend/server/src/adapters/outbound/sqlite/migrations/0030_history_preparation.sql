ALTER TABLE github_history_schedule ADD COLUMN preparation TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(preparation));
