#!/usr/bin/env python3
"""Run exposed development questions in an isolated local Answers fixture on macOS."""
import argparse
import hashlib
import json
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parent), str(HERE.parent / 'uv_benchmark')]
from development_cases import MANIFEST, read_manifest, reference_path, verify  # noqa: E402
from citation_coverage import coverage, model_context_coverage  # noqa: E402
from checkpoint import PROFILE as Q8_PROFILE, restore_into_fixture  # noqa: E402
from dogfood import (backend, prepare_answers, read_manifest as publication_manifest,
                     record_trace, save, verify_evidence, verify_quote_link,
                     wait_for_commit, wait_until)  # noqa: E402
from git_origin import FIXTURE_TOKEN, origin  # noqa: E402
from public_reference import inspect_git_tree  # noqa: E402
from run import request_answer  # noqa: E402

Q4_NAME = 'Qwen3-4B-Instruct-2507-Q4_K_M.gguf'
Q4_SHA256 = '3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597'
Q8_NAME = 'Qwen3-Embedding-0.6B-Q8_0.gguf'
Q8_SHA256 = '06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def stage_weights(source, target):
    """Clone where supported, otherwise copy into the fresh private profile."""
    if target.exists():
        raise FileExistsError(f'Weight target already exists: {target}')
    clone = subprocess.run(['/bin/cp', '-c', str(source.resolve()), str(target)],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
    if clone.returncode:
        target.unlink(missing_ok=True)
        shutil.copyfile(source, target)


def select_cases(manifest, repository, case_ids):
    available = [c for c in manifest['cases'] if c['repository'] == repository]
    known = {c['id'] for c in available}
    if case_ids and (len(set(case_ids)) != len(case_ids) or not set(case_ids) <= known):
        raise ValueError('Unknown or duplicate case ID for selected repository')
    selected = [c for c in available if not case_ids or c['id'] in case_ids]
    if not selected:
        raise ValueError('No cases selected')
    return selected


def reopen(answer, git, ref, branch):
    results = []
    for citation in (answer or {}).get('citations', []):
        try:
            hit = citation['evidence']
            if hit['trackedRef'] != 'refs/heads/' + branch:
                raise ValueError('Wrong tracked branch')
            excerpt = verify_evidence(git, hit, ref['commit'], repository=ref['repository'],
                                      repository_id=ref['repositoryId'], branch=branch)
            verify_quote_link(citation, excerpt)
            results.append({'verified': True, 'citation': citation, 'excerptUtf8': excerpt})
        except (AssertionError, KeyError, TypeError, ValueError, UnicodeError) as error:
            results.append({'verified': False, 'citation': citation, 'error': str(error)})
    return results


def capture_answer(api, request, git, ref, branch):
    started = time.monotonic()
    try:
        answer, trace_id, problem = request_answer(api, request)
    except (OSError, ValueError) as error:
        api_finished = time.monotonic()
        answer, trace_id = None, None
        problem = {'clientFailure': type(error).__name__, 'message': str(error)}
    else:
        api_finished = time.monotonic()

    reopened = []
    citation_elapsed_ms = 0
    if isinstance(answer, dict) and isinstance(answer.get('citations'), list) and answer['citations']:
        reopen_started = time.monotonic()
        reopened = reopen(answer, git, ref, branch)
        citation_elapsed_ms = (time.monotonic() - reopen_started) * 1000
    return {'elapsedMs': (api_finished - started) * 1000,
            'citationReopenElapsedMs': citation_elapsed_ms,
            'answer': answer, 'problem': problem, 'traceId': trace_id,
            'citationsReopened': reopened}


def publish(api, data, git, ref, branch):
    api('PUT', '/v1/github/credential', {'revision': 0, 'token': FIXTURE_TOKEN})
    api('PUT', '/v1/github/repositories', {'revision': 1, 'repository': ref['repository'],
        'expectedId': ref['repositoryId'], 'branch': branch, 'intervalMinutes': 1440,
        'features': {'currentCode': True, 'history': False, 'actions': False, 'codeGraph': False}})
    wait_for_commit(api, ref['commit'], timeout=1800)
    pub = publication_manifest(data, git, ref['commit'])
    if pub['publishedPaths'] != ref['expectedPublication']['paths']:
        raise ValueError('Published path count differs')
    return pub


def ready_embedding_index(api, expected_rows=None):
    state, _ = api('GET', '/v1/retrieval/index')
    if state['profile'] != Q8_PROFILE or state['enabled'] is not True:
        raise ValueError(f'Expected enabled Q8 index, got {state}')
    progress, _ = api('GET', '/v1/retrieval')
    if progress['state'] == 'disabled':
        raise ValueError(f'Q8 index unavailable after setup: {progress}')
    if progress['state'] == 'notConfigured':
        return None  # The worker may still be loading the verified staged weights.
    if progress['profile'] != Q8_PROFILE or progress['dimensions'] != 1024:
        raise ValueError(f'Unexpected Q8 index profile or dimensions: {progress}')
    if progress['state'] == 'ready' and 0 < progress['indexed'] == progress['total']:
        if expected_rows is not None and progress['total'] != expected_rows:
            raise ValueError(f'Q8 progress does not match the restored checkpoint: {progress}')
        return {'state': state, 'progress': progress}
    return None


def retrieval_route(answer, trace):
    spans = [span for span in (trace or {}).get('spans', []) if span['name'] == 'answer_retrieval']
    events = [event['name'] for span in spans for event in span.get('events', [])]
    return {'mode': (answer or {}).get('mode'), 'events': events,
            'fusionObserved': any(bool(span.get('fusionRanking')) for span in spans)}


def unavailable_control_outcome(case, answer, problem):
    """Record what the API returned without adjudicating an answer's claims."""
    if case['kind'] not in ('unavailable_runtime', 'unavailable_future'):
        return None
    if problem is not None:
        return 'request_failure'
    if answer is None:
        return 'missing_response'
    if isinstance(answer, dict):
        text = answer.get('text', '')
        if isinstance(text, str):
            if answer.get('outcome') == 'answered' and text.strip():
                return 'published_requires_review'
            if answer.get('outcome') == 'insufficientEvidence' and not text.strip() and answer.get('citations') == []:
                return 'insufficientEvidence'
    return 'malformed_response'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, default=MANIFEST, help='Working-tree benchmark manifest.')
    parser.add_argument('--repository', required=True)
    parser.add_argument('--git', type=Path, required=True, help='Pinned local bare Git object store.')
    parser.add_argument('--bundle', type=Path, required=True, help='Packaged macOS Debug app bundle.')
    parser.add_argument('--weights', type=Path, required=True, help='Pinned Qwen3-4B Q4 GGUF file.')
    parser.add_argument('--embedding-weights', type=Path,
                        help='Opt into hybrid readiness with the pinned Qwen3-Embedding 0.6B Q8 GGUF.')
    parser.add_argument('--embedding-checkpoint', type=Path,
                        help='Restore an exact pinned Q8 vector checkpoint instead of rebuilding.')
    parser.add_argument('--embedding-checkpoint-sha256', help='Expected SHA-256 of the checkpoint.')
    parser.add_argument('--report', type=Path, required=True, help='New report directory; never overwritten.')
    parser.add_argument('--case-id', action='append', help='Explicitly run only these exposed case IDs; repeat to select more.')
    args = parser.parse_args()
    hybrid = args.embedding_weights is not None
    checkpoint = args.embedding_checkpoint
    checkpoint_sha = args.embedding_checkpoint_sha256
    if (checkpoint is None) != (checkpoint_sha is None) or (checkpoint and not hybrid):
        parser.error('A checkpoint requires both checkpoint arguments and --embedding-weights.')
    if not __debug__:
        parser.error('Java qualification requires Python assertions enabled.')
    try:
        manifest = read_manifest(args.manifest)
        selected = select_cases(manifest, args.repository, args.case_id)
        ref_path = reference_path(manifest, args.repository, args.manifest)
        ref = json.loads(ref_path.read_text())
        verify(manifest, {args.repository: args.git}, args.repository, manifest_path=args.manifest)
        if digest(args.weights) != Q4_SHA256:
            raise ValueError('Local Q4 weight hash differs from pinned profile')
        if hybrid and digest(args.embedding_weights) != Q8_SHA256:
            raise ValueError('Local Q8 weight hash differs from pinned profile')
        server = args.bundle / 'Contents/MacOS/ctxfab-server'
        runtime = args.bundle / 'Contents/Helpers/Context Fabricator Runtime.app/Contents/MacOS/llama-server'
        identity = {'repository': ref['repository'], 'commit': ref['commit'],
                    'branch': manifest['fixtureBranches'][args.repository],
                    'manifestSha256': digest(args.manifest), 'referenceSha256': digest(ref_path),
                    'serverSha256': digest(server), 'runtimeSha256': digest(runtime), 'q4Sha256': digest(args.weights),
                    'cases': [c['id'] for c in selected], 'exposure': manifest['exposure'],
                    'timing': {'elapsedMs': 'Answers API round trip, including client failures',
                               'citationReopenElapsedMs': 'Local Git citation verification, zero when skipped'},
                    'embedding': {'profile': Q8_PROFILE, 'weightSha256': Q8_SHA256,
                                  'checkpointSha256': checkpoint_sha} if hybrid else None}
        args.report.mkdir(parents=True, exist_ok=False)
        save(args.report, 'identity.json', identity)
        git = inspect_git_tree(args.git, ref)
        completed = []
        submitted = []
        control_outcomes = {
            case['id']: outcome for case in selected
            if (outcome := unavailable_control_outcome(case, None, None)) is not None
        }
        try:
            with tempfile.TemporaryDirectory(prefix='ctxfab-java-dev-') as workspace, origin(
                git, repository=ref['repository'], repository_id=ref['repositoryId'], branch=identity['branch']
            ) as (port, traffic):
                data = Path(workspace) / 'data'
                data.mkdir(mode=0o700)
                inference = data / 'inference'
                inference.mkdir(parents=True, mode=0o700)
                stage_weights(args.weights, inference / Q4_NAME)
                if digest(inference / Q4_NAME) != Q4_SHA256:
                    raise ValueError('Staged Q4 weight hash differs from pinned profile')
                restored = None
                if checkpoint:
                    setup_report = args.report / 'checkpoint-publication'
                    setup_report.mkdir()
                    with backend(args.bundle, data, port, setup_report, hub_port=port, preview_runtime=False) as api:
                        pub = publish(api, data, git, ref, identity['branch'])
                    restored = restore_into_fixture(data, checkpoint, checkpoint_sha, ref_path)
                if hybrid:
                    stage_weights(args.embedding_weights, inference / Q8_NAME)
                    if digest(inference / Q8_NAME) != Q8_SHA256:
                        raise ValueError('Staged Q8 weight hash differs from pinned profile')
                with backend(args.bundle, data, port, args.report, hub_port=port, preview_runtime=False) as api:
                    if restored is not None:
                        if publication_manifest(data, git, ref['commit']) != pub:
                            raise ValueError('Restored publication changed after restart')
                    prepare_answers(api, args.report, embeddings=hybrid, expect_download=False,
                                    fixture_attempts=(lambda: len(traffic)) if restored is not None else None)
                    if restored is None:
                        pub = publish(api, data, git, ref, identity['branch'])
                    save(args.report, 'publication.json', pub)
                    if hybrid:
                        def index_ready():
                            return ready_embedding_index(api, restored)
                        initial_index = wait_until(120 if restored is not None else 1800, index_ready)
                        save(args.report, 'embedding-index.json', initial_index)
                    before = len(traffic)
                    for index, case in enumerate(selected):
                        request = {'query': case['query'], 'requestId': str(uuid.uuid4()), 'includeModelContext': True}
                        submitted.append(case['id'])
                        with (args.report / 'submitted.ndjson').open('a', encoding='utf-8') as stream:
                            stream.write(json.dumps({'id': case['id'], 'request': request}, ensure_ascii=False) + '\n')
                        row = {'id': case['id'], 'language': case['language'], 'request': request,
                               **capture_answer(api, request, git, ref, identity['branch'])}
                        answer, problem, trace_id = row['answer'], row['problem'], row['traceId']
                        answer_object = answer if isinstance(answer, dict) else None
                        if case['id'] in control_outcomes:
                            outcome = unavailable_control_outcome(case, answer, problem)
                            row['unavailableControlOutcome'] = outcome
                            control_outcomes[case['id']] = outcome
                        row['anchorCoverage'] = coverage(case, row['citationsReopened'], git, ref['commit'],
                                                         answer_object)
                        row['modelContextCoverage'] = model_context_coverage(case, row, git, ref, identity['branch'])
                        trace = None
                        if trace_id:
                            try:
                                trace = record_trace(api, trace_id, 'api_answer', args.report, f'trace-{index:03}.json')
                            except (OSError, ValueError) as error:
                                row['traceProblem'] = {'kind': type(error).__name__, 'message': str(error)}
                        row['retrieval'] = retrieval_route(answer_object, trace)
                        with (args.report / 'responses.ndjson').open('a', encoding='utf-8') as stream:
                            stream.write(json.dumps(row, ensure_ascii=False) + '\n')
                        completed.append(case['id'])
                    final = publication_manifest(data, git, ref['commit'])
                    if final != pub:
                        raise ValueError('Publication changed during run')
                    if hybrid:
                        final_index = ready_embedding_index(api, initial_index['progress']['total'])
                        save(args.report, 'embedding-index-final.json', final_index)
                        if final_index is None:
                            raise ValueError('Q8 index became incomplete during the run')
                    (args.report / 'traffic.json').write_text(json.dumps({'before': before, 'after': len(traffic)}, indent=2) + '\n')
            terminal = {'submitted': submitted, 'completed': completed, 'unsubmitted': [], 'status': 'complete'}
        except Exception as error:
            terminal = {'submitted': submitted, 'completed': completed,
                        'inFlight': [case_id for case_id in submitted if case_id not in completed],
                        'unsubmitted': [c['id'] for c in selected if c['id'] not in submitted],
                        'status': 'stopped', 'error': repr(error)}
            if control_outcomes:
                terminal['unavailableControlOutcomes'] = control_outcomes
            (args.report / 'terminal.json').write_text(json.dumps(terminal, indent=2) + '\n')
            raise
        if control_outcomes:
            terminal['unavailableControlOutcomes'] = control_outcomes
        (args.report / 'terminal.json').write_text(json.dumps(terminal, indent=2) + '\n')
        print(json.dumps(terminal, indent=2))
    except (OSError, ValueError, RuntimeError, AssertionError, subprocess.SubprocessError) as error:
        parser.exit(1, f'run failed: {error}\n')


if __name__ == '__main__':
    main()
