#!/usr/bin/env python3
"""Measure frozen Git proof coverage in paired native search captures."""
import argparse
from functools import lru_cache
import json
from pathlib import Path

from citation_coverage import _fact_coverage
from development_cases import read_manifest, reference_path, verify, witnesses
from dogfood import verify_evidence
from public_reference import inspect_git_tree
from retrieval_evaluation import MODES, identity

MAX_RANK = 20


def select_cases(manifest, repository, selection):
    cases = {c['id']: c for c in manifest['cases']
             if c['repository'] == repository and c['kind'] == 'positive'}
    if selection is None:
        selected = list(cases.values())
    else:
        if isinstance(selection, dict):
            projection = selection.get('cases')
            if not isinstance(projection, list) or any(not isinstance(c, dict) for c in projection):
                raise ValueError('Expected a question-only projection')
            for row in projection:
                identifier = row.get('id')
                if (isinstance(identifier, str) and identifier in cases and 'query' in row
                        and row['query'] != cases[identifier]['query']):
                    raise ValueError(f'{identifier}: projection question differs from frozen manifest')
            selection = [c.get('id') for c in projection]
        if not isinstance(selection, list) or any(not isinstance(i, str) for i in selection):
            raise ValueError('Expected a list of selected positive case IDs')
        if len(set(selection)) != len(selection):
            raise ValueError('Duplicate selected case ID')
        unknown = set(selection) - cases.keys()
        if unknown:
            raise ValueError(f'Unknown positive case IDs for {repository}: {sorted(unknown)}')
        selected = [cases[i] for i in selection]
    if not selected:
        raise ValueError('No positive cases selected')
    return selected


def read_capture(path):
    with path.open(encoding='utf-8') as entries:
        for line_no, line in enumerate(entries, 1):
            try:
                yield json.loads(line)
            except json.JSONDecodeError as error:
                raise ValueError(f'{path}:{line_no}: invalid search JSON') from error


def _search_pairs(cases, captures, modes):
    ids = {c['id'] for c in cases}
    if len(modes) != 2 or len(set(modes)) != 2 or any(mode not in MODES for mode in modes):
        raise ValueError('Expected two distinct native search modes')
    expected = {(identifier, mode) for identifier in ids for mode in modes}
    found = {}
    for capture in captures:
        if not isinstance(capture, dict) or not all(isinstance(capture.get(k), str) for k in ('id', 'mode')):
            raise ValueError('Malformed search capture')
        key = (capture['id'], capture['mode'])
        if key not in expected:
            raise ValueError(f'Unexpected search pair: {key}')
        if key in found:
            raise ValueError(f'Duplicate search pair: {key}')
        hits = capture.get('hits')
        trace = capture.get('traceCheck')
        if (capture.get('problem') is not None or not isinstance(hits, list) or len(hits) > MAX_RANK
                or (isinstance(trace, dict) and trace.get('fallbackObserved') is True)):
            raise ValueError(f'Invalid or failed top-{MAX_RANK} search: {key}')
        found[key] = hits
    if missing := expected - found.keys():
        raise ValueError(f'Missing search pairs: {sorted(missing)}')
    return found


def _verified_ranges(hits, git, ref, branch):
    seen = set()
    ranges = []
    for rank, hit in enumerate(hits, 1):
        try:
            location = hit['location']
            if hit['kind'] != 'current' or any(type(location[key]) is not int for key in ('startLine', 'endLine')):
                raise ValueError('Hit is not current code with exact line coordinates')
            verify_evidence(git, hit, ref['commit'], repository=ref['repository'],
                            repository_id=ref['repositoryId'], branch=branch)
            key = identity(hit)
            if key in seen:
                raise ValueError('Duplicate hit identity')
            seen.add(key)
        except (AssertionError, KeyError, TypeError, ValueError) as error:
            raise ValueError(f'hit {rank}: {error}') from error
        ranges.append(((location['path'], location['blob']), (location['startByte'], location['endByte'])))
    return ranges


def _first_complete(case, ordered_ranges, git, revision):
    sources = {}
    for rank, (source, bounds) in enumerate(ordered_ranges, 1):
        sources.setdefault(source, []).append(bounds)
        facts = _fact_coverage(case, sources, git, revision)
        if facts and all(fact['covered'] for fact in facts):
            return rank
    return None


def _group(case):
    anchors = list(witnesses(case))
    return 'README' if anchors and all(a['path'].lower() in ('readme.md', 'readme.markdown') for a in anchors) else 'code'


def _summary(rows, modes):
    first, second = modes
    summary = {}
    for name in ('all', 'README', 'code'):
        group = [r for r in rows if name == 'all' or name == r['group']]
        metrics = {'rows': len(group), 'intents': len({r['intent'] for r in group})}
        for cutoff in (5, MAX_RANK):
            covered = {mode: {r['id'] for r in group if (rank := r['modes'][mode]['firstCompleteRank'])
                             is not None and rank <= cutoff} for mode in modes}
            gains = covered[second] - covered[first]
            losses = covered[first] - covered[second]
            metrics[str(cutoff)] = {
                **{mode: len(covered[mode]) for mode in modes},
                'gains': [r['id'] for r in group if r['id'] in gains],
                'losses': [r['id'] for r in group if r['id'] in losses],
            }
        summary[name] = metrics
    return summary


def evaluate(cases, captures, git, ref, branch, modes):
    pairs = _search_pairs(cases, captures, modes)
    rows = []
    for case in cases:
        identifier = case['id']
        row = {'id': identifier, 'intent': case.get('intentId', identifier), 'group': _group(case), 'modes': {}}
        for mode in modes:
            try:
                hits = _verified_ranges(pairs[identifier, mode], git, ref, branch)
                rank = _first_complete(case, hits, git, ref['commit'])
            except ValueError as error:
                raise ValueError(f'{identifier} ({mode}): {error}') from error
            row['modes'][mode] = {'firstCompleteRank': rank}
        rows.append(row)
    return {'repository': ref['repository'], 'revision': ref['commit'], 'branch': branch, 'modes': list(modes),
            'meaning': 'Search anchor coverage only. A complete proof in search does not establish model admission or answer correctness.',
            'summary': _summary(rows, modes), 'results': rows}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--repository', required=True)
    parser.add_argument('--git', type=Path, required=True, help='Local pinned Git store')
    parser.add_argument('--searches', type=Path, required=True, help='NDJSON with one id/mode/hits row per search')
    parser.add_argument('--selection', type=Path, help='JSON list of positive case IDs or question-only projection')
    parser.add_argument('--modes', nargs=2, choices=MODES, default=('bm25', 'hybrid'))
    args = parser.parse_args()
    try:
        manifest = read_manifest(args.manifest)
        selection = json.loads(args.selection.read_text(encoding='utf-8')) if args.selection else None
        cases = select_cases(manifest, args.repository, selection)
        verify(manifest, {args.repository: args.git}, args.repository, manifest_path=args.manifest)
        ref_path = reference_path(manifest, args.repository, args.manifest)
        ref = json.loads(ref_path.read_text(encoding='utf-8'))
        git = inspect_git_tree(args.git, ref)
        # The rank prefixes repeatedly check the same immutable source anchors.
        git.blob = lru_cache(maxsize=32)(git.blob)
        report = evaluate(cases, read_capture(args.searches), git, ref,
                          manifest['fixtureBranches'][args.repository], tuple(args.modes))
    except (KeyError, ValueError, OSError) as error:
        parser.error(str(error))
    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == '__main__':
    main()
