# Public README regression benchmark

The six exposed questions cover current pinned Bytes and serde_json README documents. They include four bilingual source-backed questions and two file-scoped absence controls. The reference files pin public Git objects and the expected publication. Use local Git object stores containing each pinned commit and its first parent.

Verify both fixtures offline.

```sh
python3 quality/github/java_benchmark/development_cases.py verify \
  --manifest quality/github/readme_benchmark/readme-cases.json \
  --git-store bytes=/path/to/bytes.git \
  --git-store serde=/path/to/serde-json.git
```

For question-only projection, use `project --repository bytes --git /path/to/bytes.git` with the same manifest. The shared native runner and offline citation replay accept the same manifest and selected repository key. Their citation and API context measurements are separate from semantic grades. File-absence patterns cover narrow textual properties. Check the full pinned README to judge an answer that says a detail is absent.
