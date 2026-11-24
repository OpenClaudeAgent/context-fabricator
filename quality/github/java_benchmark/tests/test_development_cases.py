import importlib.util
import pathlib
import tempfile
import unittest

PATH = pathlib.Path(__file__).resolve().parents[1] / 'development_cases.py'
spec = importlib.util.spec_from_file_location('development_cases', PATH)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class WitnessTest(unittest.TestCase):
    def test_exact_lines_fail_when_excerpt_or_line_is_corrupted(self):
        source = b'first\nsecond\nthird\n'
        witness = {'startLine': 2, 'endLine': 2, 'text': 'second\n'}
        module.verify_span(source, witness)
        with self.assertRaises(ValueError):
            module.verify_span(source, {**witness, 'startLine': 1})
        with self.assertRaises(ValueError):
            module.verify_span(source, {**witness, 'text': 'other\n'})

    def test_byte_witness_checks_original_offsets_and_lines(self):
        source = b'first\nsecond\nthird\n'
        witness = {'startLine': 2, 'endLine': 2, 'startByte': 6,
                   'endByte': 13, 'text': 'second\n'}
        module.verify_span(source, witness)
        with self.assertRaises(ValueError):
            module.verify_span(source, {**witness, 'startByte': 7})

    def test_every_conjunct_in_every_witness_alternative_is_exposed(self):
        case = {'facts': [{'sufficientAlternatives': [[{'path': 'a'}, {'path': 'b'}],
                                                      [{'path': 'c'}]]}],
                'witnesses': [{'path': 'd'}]}
        self.assertEqual([x['path'] for x in module.witnesses(case)], ['d', 'a', 'b', 'c'])

    def test_projection_excludes_gold_and_other_repositories(self):
        manifest = {'references': {'one': 'one.json'}, 'fixtureBranches': {'one': 'fixture'},
                    'cases': [{'id': 'a', 'language': 'en', 'query': 'question',
                               'repository': 'one', 'kind': 'positive',
                               'expectation': 'secret', 'witness': {'text': 'secret'}},
                              {'id': 'b', 'language': 'fr', 'query': 'other',
                               'repository': 'two', 'expectation': 'secret'}]}
        result = module.projection(manifest, 'one', pathlib.Path('/tmp/source.git'))
        self.assertEqual(result['cases'], [{'id': 'a', 'language': 'en',
                                            'query': 'question', 'reference': 'one'}])
        self.assertNotIn('secret', str(result))
        self.assertEqual(result['references']['one']['branch'], 'fixture')

    def test_custom_reference_is_manifest_relative_and_legacy_fallback_is_named(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest_path = pathlib.Path(tmp).resolve() / 'cases.json'
            manifest = {'references': {'other': 'pins/other.json', 'jackson': 'jackson-databind-reference.json'}}
            self.assertEqual(module.reference_path(manifest, 'other', manifest_path),
                             manifest_path.parent / 'pins/other.json')
            self.assertEqual(module.reference_path(manifest, 'jackson', manifest_path),
                             PATH.parent / 'jackson-databind-reference.json')
            local = manifest_path.parent / 'jackson-databind-reference.json'
            local.touch()
            self.assertEqual(module.reference_path(manifest, 'jackson', manifest_path), local)
            with self.assertRaisesRegex(ValueError, 'Unknown repository'):
                module.reference_path(manifest, 'absent', manifest_path)

    def test_git_store_assignments_fail_on_ambiguity_or_missing_selection(self):
        manifest = {'references': {'other': 'other.json', 'jackson': 'jackson.json'}}
        store = pathlib.Path('/tmp/custom.git')
        self.assertEqual(module.git_stores(manifest, 'other', store, [], {}), {'other': store})
        self.assertEqual(module.git_stores(manifest, None, None, ['other=/tmp/custom.git'], {'jackson': store}),
                         {'other': store, 'jackson': store})
        for args in (
            (None, store, [], {}),
            ('other', store, ['other=/tmp/else.git'], {}),
            ('unknown', store, [], {}),
            ('other', None, ['unknown=/tmp/other.git'], {}),
            (None, None, ['other=/tmp/custom.git'], {}),
            (None, None, ['other='], {}),
        ):
            with self.subTest(args=args), self.assertRaises(ValueError):
                module.git_stores(manifest, *args)

    def test_absence_control_requires_complete_file(self):
        witness = {'pattern': r'Java\s+\d+', 'description': 'numeric Java version'}
        module.verify_absence(b'Java JDK required\n', witness)
        with self.assertRaises(ValueError):
            module.verify_absence(b'Java 17 required\n', witness)

if __name__ == '__main__':
    unittest.main()
