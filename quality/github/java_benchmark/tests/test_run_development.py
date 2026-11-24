import importlib.util
import pathlib
import subprocess
import sys
import unittest
import tempfile
from unittest.mock import patch

PATH = pathlib.Path(__file__).resolve().parents[1] / 'run_development.py'
spec = importlib.util.spec_from_file_location('run_development', PATH)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class SelectionTest(unittest.TestCase):
    def test_case_timing_separates_api_from_git_reopening(self):
        answer = {'citations': [{'quote': 'example'}]}
        reopened = [{'verified': False, 'error': 'example failure'}]
        with (patch.object(module, 'request_answer', return_value=(answer, 'trace', None)),
              patch.object(module, 'reopen', return_value=reopened) as reopen,
              patch.object(module.time, 'monotonic', side_effect=[1.0, 1.125, 1.25, 1.5])):
            row = module.capture_answer(None, {}, None, {}, 'validation')
        reopen.assert_called_once_with(answer, None, {}, 'validation')
        self.assertEqual(row['elapsedMs'], 125)
        self.assertEqual(row['citationReopenElapsedMs'], 250)
        self.assertEqual(row['citationsReopened'], reopened)

    def test_client_failure_preserves_api_duration_without_reopening(self):
        with (patch.object(module, 'request_answer', side_effect=OSError('offline')),
              patch.object(module, 'reopen') as reopen,
              patch.object(module.time, 'monotonic', side_effect=[1.0, 1.25])):
            row = module.capture_answer(None, {}, None, {}, 'validation')
        reopen.assert_not_called()
        self.assertEqual(row['elapsedMs'], 250)
        self.assertEqual(row['citationReopenElapsedMs'], 0)
        self.assertEqual(row['problem'], {'clientFailure': 'OSError', 'message': 'offline'})

    def test_explicit_case_filter_never_silently_omits_or_duplicates(self):
        manifest = {'cases': [{'id': 'a', 'repository': 'one'}, {'id': 'b', 'repository': 'two'}]}
        self.assertEqual([c['id'] for c in module.select_cases(manifest, 'one', ['a'])], ['a'])
        with self.assertRaises(ValueError):
            module.select_cases(manifest, 'one', ['b'])
        with self.assertRaises(ValueError):
            module.select_cases(manifest, 'one', ['a', 'a'])

    def test_clone_failure_falls_back_to_copy_and_refuses_existing_target(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            source, target = root / 'input', root / 'output'
            source.write_bytes(b'weights')
            with patch.object(module.subprocess, 'run', return_value=module.subprocess.CompletedProcess([], 1)):
                module.stage_weights(source, target)
            self.assertEqual(target.read_bytes(), b'weights')
            with self.assertRaises(FileExistsError):
                module.stage_weights(source, target)

    def test_reopen_preserves_and_checks_actual_branch(self):
        class Git:
            files = {'rev': {'A.java': {'sha': 'blob'}}}
            def blob(self, sha):
                self.asserted = sha
                return b'public class A {}\n'
        hit = {'revision': 'rev', 'trackedRef': 'refs/heads/validation',
               'source': 'github:7:A.java', 'location': {'path': 'A.java', 'blob': 'blob',
               'fileUrl': 'https://github.com/o/r/blob/rev/A.java',
               'startByte': 0, 'endByte': 18, 'startLine': 1, 'endLine': 1}}
        citation = {'evidence': hit, 'quote': 'public class A {}',
                    'lineUrl': 'https://github.com/o/r/blob/rev/A.java#L1'}
        self.assertTrue(module.reopen({'citations': [citation]}, Git(),
            {'commit': 'rev', 'repository': 'o/r', 'repositoryId': 7}, 'validation')[0]['verified'])
        self.assertFalse(module.reopen({'citations': [citation]}, Git(),
            {'commit': 'rev', 'repository': 'o/r', 'repositoryId': 7}, 'master')[0]['verified'])
        self.assertEqual(hit['trackedRef'], 'refs/heads/validation')

    def test_optimized_runner_fails_closed_for_default_bm25(self):
        result = subprocess.run([
            sys.executable, '-O', str(PATH), '--repository', 'jackson', '--git', 'unused',
            '--bundle', 'unused', '--weights', 'unused', '--report', 'unused',
        ], text=True, capture_output=True)
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn('requires Python assertions enabled', result.stderr)

    def test_hybrid_readiness_requires_exact_active_profile_and_full_nonempty_index(self):
        state = {'enabled': True, 'profile': module.Q8_PROFILE, 'epoch': 0}
        progress = {'profile': module.Q8_PROFILE, 'dimensions': 1024, 'state': 'incomplete',
                    'indexed': 2, 'total': 3}
        api = lambda _, route: (state if route.endswith('/index') else progress, None)
        self.assertIsNone(module.ready_embedding_index(api))
        progress.clear()
        progress['state'] = 'notConfigured'
        self.assertIsNone(module.ready_embedding_index(api))
        progress.update(profile=module.Q8_PROFILE, dimensions=1024)
        progress.update(state='ready', indexed=0, total=0)
        self.assertIsNone(module.ready_embedding_index(api))
        progress.update(indexed=3, total=3)
        self.assertEqual(module.ready_embedding_index(api, 3)['progress'], progress)
        with self.assertRaises(ValueError):
            module.ready_embedding_index(api, 4)
        progress['profile'] = 'different'
        with self.assertRaises(ValueError):
            module.ready_embedding_index(api)
        state['enabled'] = False
        with self.assertRaises(ValueError):
            module.ready_embedding_index(api)

    def test_route_receipt_keeps_fallback_and_failed_generation_distinct(self):
        trace = {'spans': [{'name': 'answer_retrieval', 'events': [
            {'name': 'Semantic index incomplete'}, {'name': 'Fell back to BM25'}]}]}
        receipt = module.retrieval_route({'mode': 'bm25'}, trace)
        self.assertEqual(receipt['mode'], 'bm25')
        self.assertIn('Semantic index incomplete', receipt['events'])
        trace['spans'][0].update(fusionRanking={'rows': 5}, events=[{'name': 'Hybrid lookup selected'}])
        receipt = module.retrieval_route(None, trace)
        self.assertIsNone(receipt['mode'])
        self.assertTrue(receipt['fusionObserved'])
        self.assertEqual(receipt['events'], ['Hybrid lookup selected'])

    def test_unavailable_controls_record_outcomes_without_grading_publications(self):
        cases = (
            (None, None, 'missing_response'),
            (None, {'status': 503}, 'request_failure'),
            ({'outcome': 'insufficientEvidence', 'citations': []}, None, 'insufficientEvidence'),
            ({'outcome': 'answered', 'text': 'No', 'citations': []}, None, 'published_requires_review'),
            ({'outcome': 'answered', 'text': '  '}, None, 'malformed_response'),
            ({'outcome': 'insufficientEvidence', 'text': 'No', 'citations': []}, None, 'malformed_response'),
            ({'outcome': 'insufficientEvidence', 'citations': [{'quote': 'test'}]}, None, 'malformed_response'),
            ({}, None, 'malformed_response'),
            ([], None, 'malformed_response'),
        )
        for kind in ('unavailable_runtime', 'unavailable_future'):
            for answer, problem, expected in cases:
                with self.subTest(kind=kind, answer=answer, problem=problem):
                    self.assertEqual(module.unavailable_control_outcome({'kind': kind}, answer, problem), expected)
        for kind in ('positive', 'absent_in_file', 'absent_path', 'absent_namespace'):
            self.assertIsNone(module.unavailable_control_outcome(
                {'kind': kind}, {'outcome': 'answered', 'text': 'No'}, None))

    def test_setup_counts_only_non_fixture_outbound_attempts(self):
        status = {'outboundAttempts': 186}
        operation = None

        def api(method, route, body=None):
            nonlocal operation
            if route == '/v1/runtime':
                value = {'state': 'ready'}
            elif route == '/v1/status':
                value = status
            elif route.endswith('/plan'):
                value = {'canStart': True, 'runtimeRequired': False, 'fingerprint': 'pin',
                         'selection': body}
            elif route.endswith('/setup') and method == 'POST':
                operation = body['operationId']
                value = {'operationId': operation}
            elif route.endswith('/setup'):
                value = {'operationId': operation, 'phase': 'completed', 'answerReady': True}
            else:
                self.assertEqual(route, '/v1/generation/models')
                value = {'activeProfile': 'qwen3-4b-instruct-2507-q4-k-m-3605803b-v1'}
            return value, None

        with tempfile.TemporaryDirectory() as temp:
            report = pathlib.Path(temp)
            self.assertEqual(module.prepare_answers(
                api, report, expect_download=False, fixture_attempts=lambda: 186), 0)
            status['outboundAttempts'] += 1
            with self.assertRaises(AssertionError):
                module.prepare_answers(api, report, expect_download=False, fixture_attempts=lambda: 186)

if __name__ == '__main__':
    unittest.main()
