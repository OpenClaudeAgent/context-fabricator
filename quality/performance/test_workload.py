import sqlite3
import unittest
from contextlib import closing
from pathlib import Path
from tempfile import TemporaryDirectory

from workload import persisted_vectors


class PersistedVectorsTest(unittest.TestCase):
    def test_fingerprint_binds_vectors_to_profile_and_pinned_passage(self):
        with (
            TemporaryDirectory() as directory,
            closing(sqlite3.connect(Path(directory) / "context.sqlite3")) as connection,
        ):
            connection.executescript("""
                CREATE TABLE documents (
                    id INTEGER PRIMARY KEY, source TEXT, revision TEXT,
                    tracked_ref TEXT, passage_start INTEGER
                );
                CREATE TABLE document_vectors (document_id INTEGER, profile TEXT, embedding BLOB);
                INSERT INTO documents VALUES
                    (7, 'src/a.rs', 'pin', 'refs/heads/main', 0),
                    (8, 'src/b.rs', 'pin', 'refs/heads/main', 64);
                INSERT INTO document_vectors VALUES
                    (7, 'primary', X'0102'),
                    (8, 'alternate', X'0102');
            """)
            data = Path(directory)
            original = persisted_vectors(data, "primary")
            self.assertEqual(original["count"], 1)
            self.assertEqual(persisted_vectors(data, "missing")["count"], 0)
            connection.execute("UPDATE document_vectors SET embedding = X'0304' WHERE document_id = 8")
            connection.commit()
            self.assertEqual(persisted_vectors(data, "primary"), original)
            connection.execute("UPDATE documents SET revision = 'new-pin' WHERE id = 7")
            connection.commit()
            self.assertNotEqual(persisted_vectors(data, "primary")["sha256"], original["sha256"])
            connection.execute("UPDATE documents SET revision = 'pin' WHERE id = 7")
            connection.execute("UPDATE document_vectors SET embedding = X'0506' WHERE document_id = 7")
            connection.commit()
            self.assertNotEqual(persisted_vectors(data, "primary")["sha256"], original["sha256"])


if __name__ == "__main__":
    unittest.main()
