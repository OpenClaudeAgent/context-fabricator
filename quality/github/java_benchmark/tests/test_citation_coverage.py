import copy
import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from citation_coverage import coverage as raw_coverage, replay_response
from run_development import reopen


def coverage(case, reopened, git, revision):
    answer = {"outcome": "answered", "text": "An answer.",
              "citations": [item["citation"] for item in reopened]}
    return raw_coverage(case, reopened, git, revision, answer)


class Git:
    def __init__(self, text):
        self.text = text
        self.files = {'rev': {'A.java': {'sha': 'blob'}}}

    def blob(self, sha):
        assert sha == 'blob'
        return self.text


def cited(source, quote, line, *, verified=True, blob='blob'):
    loc = {'path': 'A.java', 'blob': blob, 'startByte': 0, 'endByte': len(source),
           'fileUrl': 'https://example.test/A.java'}
    return {'verified': verified, 'excerptUtf8': source.decode(), 'citation': {
        'evidence': {'revision': 'rev', 'location': loc}, 'quote': quote,
        'lineUrl': loc['fileUrl'] + '#L' + str(line)}}


class CoverageTest(unittest.TestCase):
    def test_union_and_alternatives_require_published_exact_ranges(self):
        source = b'first\nsecond\nthird\n'; git = Git(source)
        def a(start, end, text):
            return {'path': 'A.java', 'blob': 'blob', 'startLine': start,
                    'endLine': end, 'text': text}
        case = {'kind': 'positive', 'facts': [{'sufficientAlternatives': [
            [a(1, 1, 'first\n'), a(2, 2, 'second\n')], [a(3, 3, 'third\n')]]}]}
        self.assertEqual(coverage(case, [cited(source, 'first', 1), cited(source, 'second', 2)], git, 'rev')['status'], 'anchors_covered')
        self.assertEqual(coverage(case, [cited(source, 'first', 1)], git, 'rev')['status'], 'human_review')
        self.assertEqual(coverage(case, [cited(source, 'third', 3)], git, 'rev')['status'], 'anchors_covered')
        self.assertEqual(coverage(case, [cited(source, 'third', 3, verified=False)], git, 'rev')['status'], 'human_review')
        self.assertEqual(coverage(case, [cited(source, 'third', 3, blob='other')], git, 'rev')['status'], 'human_review')

    def test_unpublished_or_detached_quotes_and_internal_whitespace(self):
        source = b'  return "two words";  \n'
        git = Git(source)
        case = {'kind': 'positive', 'witness': {'path': 'A.java', 'blob': 'blob',
                'startLine': 1, 'endLine': 1, 'text': source.decode()}}
        full = cited(source, 'return "two words";', 1)
        self.assertEqual(raw_coverage(case, [full], git, 'rev', {'outcome': 'insufficientEvidence', 'citations': [full['citation']]})['status'], 'human_review')
        self.assertEqual(raw_coverage(case, [full], git, 'rev', {'outcome': 'answered', 'citations': []})['status'], 'human_review')
        self.assertEqual(raw_coverage(case, [full], git, 'rev', {
            'outcome': 'answered', 'text': '  ', 'citations': [full['citation']]
        })['status'], 'human_review')
        self.assertEqual(coverage(case, [cited(source, 'return "two', 1), cited(source, 'words";', 1)], git, 'rev')['status'], 'human_review')
        self.assertEqual(coverage(case, [full], git, 'rev')['status'], 'anchors_covered')

    def test_duplicate_same_line_is_not_resolved_by_line_link(self):
        source = b'return x; return x;\n'; git = Git(source)
        case = {'kind': 'positive', 'witness': {'path': 'A.java', 'blob': 'blob',
                'startLine': 1, 'endLine': 1, 'text': source.decode()}}
        self.assertEqual(coverage(case, [cited(source, 'return x;', 1)], git, 'rev')['status'], 'human_review')

    def test_replay_rejects_untrusted_origin_span_and_quote_links(self):
        source = b'return x;\n'
        git = Git(source)
        ref = {'commit': 'rev', 'repository': 'o/r', 'repositoryId': 7}
        case = {'kind': 'positive', 'witness': {'path': 'A.java', 'blob': 'blob',
                'startLine': 1, 'endLine': 1, 'text': source.decode()}}
        citation = cited(source, 'return x;', 1)['citation']
        citation['evidence'].update(source='github:7:A.java', trackedRef='refs/heads/main')
        location = citation['evidence']['location']
        location.update(startLine=1, endLine=1, fileUrl='https://github.com/o/r/blob/rev/A.java')
        citation['lineUrl'] = location['fileUrl'] + '#L1'
        row = {'id': 'positive', 'answer': {'outcome': 'answered', 'text': 'An answer.'}}
        for field, value in (
            ('source', 'github:8:A.java'), ('revision', 'other'), ('trackedRef', 'refs/heads/other'),
            ('blob', 'other'), ('fileUrl', 'https://example.test/A.java'), ('endByte', len(source) + 1),
            ('startLine', 2), ('endLine', 2), ('quote', 'not present'), ('lineUrl', 'https://example.test/#L1'),
        ):
            with self.subTest(field=field):
                item = copy.deepcopy(citation)
                target = (item if field in item else item['evidence']
                          if field in item['evidence'] else item['evidence']['location'])
                target[field] = value
                if field == 'fileUrl':
                    item['lineUrl'] = value + '#L1'
                row['answer']['citations'] = [item]
                self.assertFalse(reopen(row['answer'], git, ref, 'main')[0]['verified'])
                self.assertEqual(replay_response(case, row, git, ref, 'main')['coverage']['status'], 'human_review')
        row['answer']['citations'] = [citation]
        self.assertEqual(replay_response(case, row, git, ref, 'main')['coverage']['status'], 'anchors_covered')

    def test_overlapping_quote_cannot_claim_a_unique_line_link(self):
        source = b'aaaa\n'
        git = Git(source)
        citation = cited(source, 'aaa', 1)['citation']
        citation['evidence'].update(source='github:7:A.java', trackedRef='refs/heads/main')
        location = citation['evidence']['location']
        location.update(startLine=1, endLine=1, fileUrl='https://github.com/o/r/blob/rev/A.java')
        citation['lineUrl'] = location['fileUrl'] + '#L1'
        ref = {'commit': 'rev', 'repository': 'o/r', 'repositoryId': 7}
        self.assertFalse(reopen({'citations': [citation]}, git, ref, 'main')[0]['verified'])
        del citation['lineUrl']
        self.assertTrue(reopen({'citations': [citation]}, git, ref, 'main')[0]['verified'])
        citation['quote'] = ''
        self.assertFalse(reopen({'citations': [citation]}, git, ref, 'main')[0]['verified'])

    def test_model_context_pinned_coverage_is_separate_from_publication(self):
        git = Git(b'  first\n  second\n')
        case = {'id': 'one', 'query': 'Which?', 'kind': 'positive', 'witness': {
            'path': 'A.java', 'blob': 'blob', 'startLine': 1, 'endLine': 2,
            'text': '  first\n  second\n'}}
        ref = {'commit': 'rev', 'repository': 'o/r'}
        slot = {'slot': 1, 'kind': 'current', 'repository': 'o/r', 'branch': 'main',
                'parts': [{'path': 'A.java', 'revision': 'rev', 'startLine': 1, 'endLine': 2,
                           'text': 'first\n  second'}]}
        row = {'id': 'one', 'requestId': 'opaque', 'request': {'query': 'Which?', 'requestId': 'opaque'},
               'answer': {'outcome': 'insufficientEvidence', 'modelContext': {
                   'question': 'Which?', 'sources': [slot]}}}
        result = replay_response(case, row, git, ref, 'main')
        self.assertEqual(result['coverage']['status'], 'human_review')
        self.assertEqual(result['modelContextCoverage']['status'], 'anchors_covered')
        mutations = (
            ('id', 'different'), ('requestId', 'wrong'),
            ('question', 'Different question'), ('repository', 'o/wrong'),
            ('branch', 'wrong'), ('revision', 'wrong'), ('startLine', 2),
            ('text', 'first\n  incorrect'),
        )
        for key, value in mutations:
            with self.subTest(key=key):
                item = copy.deepcopy(row)
                ctx = item['answer']['modelContext']
                source = ctx['sources'][0]
                part = source['parts'][0]
                target = next(obj for obj in (item, ctx, source, part) if key in obj)
                target[key] = value
                result = replay_response(case, item, git, ref, 'main')
                self.assertEqual(result['modelContextCoverage']['status'], 'unavailable')
        row['answer']['modelContext']['sources'] = []
        self.assertEqual(replay_response(case, row, git, ref, 'main')['modelContextCoverage']['status'], 'verified')
        del row['answer']['modelContext']
        self.assertEqual(replay_response(case, row, git, ref, 'main')['modelContextCoverage']['reason'], 'missing_context')
        row['problem'] = {'status': 503}
        self.assertEqual(replay_response(case, row, git, ref, 'main')['modelContextCoverage']['status'], 'request_failure')

    def test_width_missing_default_anchor_is_not_covered(self):
        source = b'private int maxWidth = UNSET;\nstatic final int UNSET = MAX;\nDEFAULT = builder();\n'; git = Git(source)
        anchors = [{'path': 'A.java', 'blob': 'blob', 'startLine': n, 'endLine': n, 'text': t}
                   for n, t in enumerate(source.decode().splitlines(keepends=True), 1)]
        case = {'kind': 'positive', 'facts': [{'sufficientAlternatives': [anchors]}]}
        cites = [cited(source, 'private int maxWidth = UNSET;', 1), cited(source, 'static final int UNSET = MAX;', 2)]
        self.assertEqual(coverage(case, cites, git, 'rev')['status'], 'human_review')
        cites.append(cited(source, 'DEFAULT = builder();', 3))
        self.assertEqual(coverage(case, cites, git, 'rev')['status'], 'anchors_covered')

    def test_archived_unavailable_publication_is_distinct_from_withholding(self):
        case = {'kind': 'unavailable_runtime'}
        for answer, expected in (
            ({'outcome': 'insufficientEvidence', 'citations': []}, 'insufficientEvidence'),
            ({'outcome': 'answered', 'text': 'No', 'citations': []}, 'published_requires_review'),
            ([], 'malformed_response'),
        ):
            with self.subTest(expected=expected):
                result = replay_response(case, {'id': 'runtime', 'answer': answer}, Git(b''), {'commit': 'rev'}, 'main')
                self.assertEqual(result['unavailableControlOutcome'], expected)
                self.assertEqual(result['coverage']['status'], 'not_applicable')
        result = replay_response({'kind': 'absent_path'}, {'id': 'missing', 'answer': None},
                                 Git(b''), {'commit': 'rev'}, 'main')
        self.assertNotIn('unavailableControlOutcome', result)
