"""Check the requirement IDs that code, tests and kiln cite (CLAUDE.md, Requirement IDs)."""

from collections import Counter
from pathlib import Path
import tomllib
import unittest


class RequirementRegistry(unittest.TestCase):
    def test_every_id_is_unique(self):
        path = Path(__file__).resolve().parents[1] / "docs" / "requirements.toml"
        with path.open("rb") as registry:
            pending = [tomllib.load(registry)]
        counts = Counter()
        while pending:
            item = pending.pop()
            if isinstance(item, dict):
                if "id" in item:
                    counts[item["id"]] += 1
                pending.extend(item.values())
            elif isinstance(item, list):
                pending.extend(item)

        self.assertIn("CP-T1.10", counts)
        duplicates = sorted(key for key, count in counts.items() if count > 1)
        self.assertEqual(duplicates, [], f"duplicate IDs in {path.name}: {', '.join(duplicates)}")


if __name__ == "__main__":
    unittest.main()
