"""Generate syntax boundary hints over the unchanged reference source bytes."""

import json
import sys
from pathlib import Path

import tree_sitter_python
import tree_sitter_rust
from tree_sitter import Language, Parser

GRAMMARS = {
    ".py": Language(tree_sitter_python.language()),
    ".rs": Language(tree_sitter_rust.language()),
}


def boundaries(path, source, budget):
    language = GRAMMARS.get(path.suffix)
    if language is None:
        return []
    root = Parser(language).parse(source).root_node
    if root.has_error:
        return []

    # Descend into an oversized node. Otherwise keep it intact and offer
    # its ending only when the rest of the original line is whitespace.
    candidates = list(root.named_children)
    ends = set()
    while candidates:
        node = candidates.pop()
        if node.end_byte - node.start_byte > budget and node.named_child_count:
            candidates.extend(node.named_children)
            continue
        line_stop = source.find(b"\n", node.end_byte)
        next_offset = len(source) if line_stop == -1 else line_stop + 1
        if not source[node.end_byte:next_offset].strip(b" \t\r\n"):
            ends.add(next_offset)
    return sorted(ends)


def main():
    directory, manifest, output = map(Path, sys.argv[1:])
    result = []
    for reference in json.loads(manifest.read_text(encoding="utf-8")):
        path = Path(reference["path"])
        original = (directory / reference["file"]).read_bytes()
        extension = b"# " if path.suffix == ".py" else b"// "
        insertion = extension + b"fixture-only UTF-8 note: \xc3\xa9\n"
        edited = insertion + original
        # These reference paths fit within the 2,048-byte publication target.
        budget = min(2048, 3072 - len(reference["path"].encode()) - 1)
        result.append(
            {
                "path": reference["path"],
                "boundaries": boundaries(path, original, budget),
                "insertion": insertion.decode(),
                "editedBoundaries": boundaries(path, edited, budget),
            }
        )
    output.write_text(json.dumps(result), encoding="utf-8")


if __name__ == "__main__":
    main()
