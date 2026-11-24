import copy
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from paired_source_recall import evaluate, select_cases


TEXT = b'alpha\nbeta\ngamma\n'
REF = {'commit': 'pin', 'repository': 'example/demo', 'repositoryId': 23}


class Git:
    files = {'pin': {'A.java': {'sha': 'blob'}}}

    def blob(self, sha):
        if sha != 'blob':
            raise LookupError(sha)
        return TEXT


def hit(first, last):
    intervals = [0, 6, 11, 17]
    return {
        'source': 'github:23:A.java', 'kind': 'current',
        'revision': 'pin', 'trackedRef': 'refs/heads/main',
        'location': {'path': 'A.java', 'blob': 'blob',
                     'fileUrl': 'https://github.com/example/demo/blob/pin/A.java',
                     'startLine': first, 'endLine': last,
                     'startByte': intervals[first - 1], 'endByte': intervals[last]},
    }


def witness(line, text):
    return {'path': 'A.java', 'blob': 'blob', 'startLine': line, 'endLine': line, 'text': text}


def case(identifier='a'):
    return {
        'id': identifier, 'query': 'What changes?', 'repository': 'demo', 'kind': 'positive',
        'facts': [{'sufficientAlternatives': [
            [witness(1, 'alpha\n'), witness(2, 'beta\n')], [witness(3, 'gamma\n')],
        ]}],
    }


def capture(mode, hits, identifier='a'):
    return {'id': identifier, 'mode': mode, 'hits': hits}


class PairedSourceRecallTest(unittest.TestCase):
    def evaluate(self, rows):
        return evaluate([case()], rows, Git(), REF, 'main', ('bm25', 'hybrid'))

    def test_first_complete_rank_uses_alternatives_and_union(self):
        data = self.evaluate([
            capture('bm25', [hit(1, 1), hit(2, 2)]),
            capture('hybrid', [hit(3, 3)]),
        ])
        row, = data['results']
        self.assertEqual(row['modes']['bm25']['firstCompleteRank'], 2)
        self.assertEqual(row['modes']['hybrid']['firstCompleteRank'], 1)
        self.assertEqual(data['summary']['all']['5']['hybrid'], 1)

    def test_explicit_subset_and_strict_pairing(self):
        manifest = {'cases': [case('a'), case('b')]}
        self.assertEqual(select_cases(manifest, 'demo', ['b']), [case('b')])
        self.assertEqual(select_cases(manifest, 'demo', {'cases': [{'id': 'b', 'query': 'What changes?'}]}), [case('b')])
        for selection in ([], ['b', 'b'], ['missing'], {'cases': [{'id': 'a', 'query': 'wrong'}]}):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                select_cases(manifest, 'demo', selection)
        original = capture('bm25', [hit(1, 1)])
        with self.assertRaisesRegex(ValueError, 'Missing'):
            self.evaluate([original])
        with self.assertRaisesRegex(ValueError, 'Duplicate'):
            self.evaluate([original, original])
        with self.assertRaisesRegex(ValueError, 'Unexpected'):
            self.evaluate([original, capture('hybrid', [], 'b')])
        with self.assertRaisesRegex(ValueError, 'Duplicate'):
            self.evaluate([capture('bm25', [hit(1, 1), hit(1, 1)]), capture('hybrid', [])])

    def test_uses_canonical_encoded_git_urls(self):
        git = Git()
        git.files = {'pin': {'A space.java': {'sha': 'blob'}}}
        row = case()
        for group in row['facts'][0]['sufficientAlternatives']:
            for anchor in group:
                anchor['path'] = 'A space.java'
        source = hit(3, 3)
        source['source'] = 'github:23:A space.java'
        source['location'].update(path='A space.java',
                                  fileUrl='https://github.com/example/demo/blob/pin/A%20space.java')
        result = evaluate([row], [capture('bm25', []), capture('hybrid', [source])],
                          git, REF, 'main', ('bm25', 'hybrid'))
        self.assertEqual(result['results'][0]['modes']['hybrid']['firstCompleteRank'], 1)

    def test_validates_hits_after_first_complete_proof(self):
        mutations = [
            ('revision', 'wrong'), ('trackedRef', 'refs/heads/other'),
            ('source', 'github:24:A.java'), ('kind', 'history'),
            ('location.path', 'B.java'), ('location.blob', 'wrong'),
            ('location.endByte', 100), ('location.startLine', True),
            ('location.fileUrl', 'https://github.com/example/other/blob/pin/A.java'),
        ]
        for field, value in mutations:
            changed = copy.deepcopy(hit(1, 1))
            obj = changed
            if '.' in field:
                field = field.split('.')[1]
                obj = changed['location']
            obj[field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'hit 2'):
                self.evaluate([capture('bm25', [hit(3, 3), changed]), capture('hybrid', [])])
